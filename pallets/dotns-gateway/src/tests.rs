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

//! Tests for the dotNS gateway pallet.

use frame_support::{
	assert_noop, assert_ok,
	traits::{GetStorageVersion, OnRuntimeUpgrade, StorageVersion},
};

use crate::{
	migration::MigrateV0ToV1,
	mock::*,
	pallet::{
		AccountAlias, AccountNames, AliasRegistration, AttestationAllowance, DispatcherAddress,
		Error, LiteLabelOwner,
	},
	types::{AccountNameRecord, BaseLabel, ChatKey, Collection, DispatcherRevert, Link, NameEntry},
	weights::WeightInfo,
};
use sp_core::H160;
use sp_runtime::{transaction_validity::TransactionSource, DispatchError};

const ALICE: u64 = 1;
const BOB: u64 = 2;
const ATTESTER: u64 = 10;
/// Default mock Unix time (matches `MOCK_NOW` initial value).
const MOCK_NOW_BASE: u64 = 1_700_000_000;
/// A `signed_at` value equal to mock "now" — the ordinary happy-path fixture.
const SIGNED_AT_NOW: u64 = MOCK_NOW_BASE;

fn alias_a() -> [u8; 32] {
	[1u8; 32]
}

fn base_name(s: &[u8]) -> BaseLabel {
	BaseLabel::try_from(s.to_vec()).unwrap()
}

// Canonical fixtures matching `StringUtils.isLitePersonLabel` / `isSingleLabel`.
const ALICE_LITE: &[u8] = b"alice.42";
const BOB_LITE: &[u8] = b"bob.42";
const ALICE_BASE: &[u8] = b"alice";
const BOB_BASE: &[u8] = b"bob";
const ALICE_FULL: &[u8] = b"alicefull";

fn default_chat_key() -> ChatKey {
	ChatKey::from([0xAB; 65])
}

/// A chat key distinct from [`default_chat_key`], for records seeded as if by a lite reservation.
fn lite_chat_key() -> ChatKey {
	ChatKey::from([0xCD; 65])
}

fn entry(label: &[u8], chat: Option<ChatKey>) -> Option<NameEntry> {
	Some(NameEntry { label: base_name(label), chat })
}

mod attestation_allowance {
	use super::*;

	#[test]
	fn increase_attestation_allowance_succeeds() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);

			assert_ok!(DotnsGateway::increase_attestation_allowance(
				RuntimeOrigin::root(),
				ATTESTER,
				5,
			));

			assert_eq!(AttestationAllowance::<Test>::get(ATTESTER), 5);

			System::assert_last_event(
				crate::Event::<Test>::AttestationAllowanceIncreased { account: ATTESTER, count: 5 }
					.into(),
			);
		});
	}

	#[test]
	fn increase_is_saturating() {
		new_test_ext().execute_with(|| {
			set_attestation_allowance(ATTESTER, u32::MAX - 1);

			assert_ok!(DotnsGateway::increase_attestation_allowance(
				RuntimeOrigin::root(),
				ATTESTER,
				10,
			));

			assert_eq!(AttestationAllowance::<Test>::get(ATTESTER), u32::MAX);
		});
	}

	#[test]
	fn increase_requires_manager_origin() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				DotnsGateway::increase_attestation_allowance(
					RuntimeOrigin::signed(ALICE),
					ATTESTER,
					5,
				),
				DispatchError::BadOrigin
			);
		});
	}

	#[test]
	fn clear_attestation_allowance_succeeds() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			set_attestation_allowance(ATTESTER, 10);

			assert_ok!(DotnsGateway::clear_attestation_allowance(RuntimeOrigin::root(), ATTESTER,));

			assert_eq!(AttestationAllowance::<Test>::get(ATTESTER), 0);

			System::assert_last_event(
				crate::Event::<Test>::AllAttestationAllowanceCleared { attester: ATTESTER }.into(),
			);
		});
	}

	#[test]
	fn clear_requires_manager_origin() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				DotnsGateway::clear_attestation_allowance(RuntimeOrigin::signed(ALICE), ATTESTER,),
				DispatchError::BadOrigin
			);
		});
	}
}

mod dispatcher_address {
	use super::*;

	#[test]
	fn set_dispatcher_address_succeeds() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			let new_addr = H160([0xAB; 20]);

			assert_ok!(DotnsGateway::set_dispatcher_address(RuntimeOrigin::root(), new_addr));

			assert_eq!(DispatcherAddress::<Test>::get(), Some(new_addr));
			System::assert_last_event(
				crate::Event::<Test>::DispatcherAddressSet { address: new_addr }.into(),
			);
		});
	}

	#[test]
	fn set_dispatcher_address_requires_manager_origin() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				DotnsGateway::set_dispatcher_address(
					RuntimeOrigin::signed(ALICE),
					H160([0xAB; 20]),
				),
				DispatchError::BadOrigin
			);
		});
	}

	#[test]
	fn set_dispatcher_address_overwrites() {
		new_test_ext().execute_with(|| {
			let first = H160([0x11; 20]);
			let second = H160([0x22; 20]);

			assert_ok!(DotnsGateway::set_dispatcher_address(RuntimeOrigin::root(), first));
			assert_ok!(DotnsGateway::set_dispatcher_address(RuntimeOrigin::root(), second));

			assert_eq!(DispatcherAddress::<Test>::get(), Some(second));
		});
	}

	#[test]
	fn reserve_name_fails_when_address_unset() {
		new_test_ext().execute_with(|| {
			DispatcherAddress::<Test>::kill();
			set_attestation_allowance(ATTESTER, 5);

			assert_noop!(
				DotnsGateway::reserve_name(
					RuntimeOrigin::signed(ATTESTER),
					ALICE,
					valid_candidate_signature(ALICE),
					base_name(ALICE_LITE),
					default_chat_key(),
					None,
					SIGNED_AT_NOW,
				),
				Error::<Test>::DispatcherAddressNotSet
			);
		});
	}

	#[test]
	fn register_name_fails_when_address_unset() {
		new_test_ext().execute_with(|| {
			DispatcherAddress::<Test>::kill();

			let label = base_name(ALICE_BASE);
			let link = Link::None(default_chat_key());

			assert_noop!(
				DotnsGateway::register_name(
					person_registration_origin(alias_a()),
					ALICE,
					label,
					link,
				),
				Error::<Test>::DispatcherAddressNotSet
			);
		});
	}

	#[test]
	fn call_dispatcher_routes_to_set_address() {
		new_test_ext().execute_with(|| {
			let new_addr = H160([0x77; 20]);
			assert_ok!(DotnsGateway::set_dispatcher_address(RuntimeOrigin::root(), new_addr));
			set_attestation_allowance(ATTESTER, 5);

			assert_ok!(DotnsGateway::reserve_name(
				RuntimeOrigin::signed(ATTESTER),
				ALICE,
				valid_candidate_signature(ALICE),
				base_name(ALICE_LITE),
				default_chat_key(),
				None,
				SIGNED_AT_NOW,
			));

			let calls = get_contract_calls();
			assert_eq!(calls.len(), 1);
			assert_eq!(calls[0].0, new_addr);
		});
	}
}

mod reservation {
	use super::*;

	#[test]
	fn username_reservation_succeeds() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			set_attestation_allowance(ATTESTER, 5);

			let result = DotnsGateway::reserve_name(
				RuntimeOrigin::signed(ATTESTER),
				ALICE,
				valid_candidate_signature(ALICE),
				base_name(ALICE_LITE),
				default_chat_key(),
				None,
				SIGNED_AT_NOW,
			);

			assert_ok!(&result);

			// Attestation allowance decremented.
			assert_eq!(AttestationAllowance::<Test>::get(ATTESTER), 4);

			// Contract call dispatched to dispatcher address.
			let calls = get_contract_calls();
			assert_eq!(calls.len(), 1);
			assert_eq!(calls[0].0, DispatcherAddr::get());
			assert_eq!(calls[0].2, 0);

			assert_eq!(
				AccountNames::<Test>::get(ALICE),
				Some(AccountNameRecord {
					lite: entry(ALICE_LITE, Some(default_chat_key())),
					full: None
				})
			);

			System::assert_last_event(
				crate::Event::<Test>::NameReserved {
					candidate: ALICE,
					attester: ATTESTER,
					lite_label: base_name(ALICE_LITE),
					chat_key: default_chat_key(),
					reserved_base_label: None,
				}
				.into(),
			);

			// Reservation is free for the caller.
			let info = result.unwrap();
			assert_eq!(info.pays_fee, frame_support::dispatch::Pays::No);
		});
	}

	#[test]
	fn later_reservation_overwrites_account_lite_label() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			set_attestation_allowance(ATTESTER, 5);

			assert_ok!(DotnsGateway::reserve_name(
				RuntimeOrigin::signed(ATTESTER),
				ALICE,
				valid_candidate_signature(ALICE),
				base_name(ALICE_LITE),
				default_chat_key(),
				None,
				SIGNED_AT_NOW
			));
			assert_ok!(DotnsGateway::reserve_name(
				RuntimeOrigin::signed(ATTESTER),
				ALICE,
				valid_candidate_signature(ALICE),
				base_name(BOB_LITE),
				default_chat_key(),
				None,
				SIGNED_AT_NOW
			));

			assert_eq!(
				AccountNames::<Test>::get(ALICE),
				Some(AccountNameRecord {
					lite: entry(BOB_LITE, Some(default_chat_key())),
					full: None
				})
			);
		});
	}

	#[test]
	fn reservation_preserves_registered_full_label() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			set_attestation_allowance(ATTESTER, 5);
			AccountNames::<Test>::insert(
				ALICE,
				AccountNameRecord { lite: None, full: entry(ALICE_BASE, Some(lite_chat_key())) },
			);

			assert_ok!(DotnsGateway::reserve_name(
				RuntimeOrigin::signed(ATTESTER),
				ALICE,
				valid_candidate_signature(ALICE),
				base_name(ALICE_LITE),
				default_chat_key(),
				None,
				SIGNED_AT_NOW
			));

			assert_eq!(
				AccountNames::<Test>::get(ALICE),
				Some(AccountNameRecord {
					lite: entry(ALICE_LITE, Some(default_chat_key())),
					full: entry(ALICE_BASE, Some(lite_chat_key()))
				})
			);
		});
	}

	#[test]
	fn fails_without_attestation_allowance() {
		new_test_ext().execute_with(|| {
			// ATTESTER has no allowance.
			assert_noop!(
				DotnsGateway::reserve_name(
					RuntimeOrigin::signed(ATTESTER),
					ALICE,
					valid_candidate_signature(ALICE),
					base_name(ALICE_LITE),
					default_chat_key(),
					None,
					SIGNED_AT_NOW,
				),
				Error::<Test>::NoAttestationAllowance
			);
		});
	}

	#[test]
	fn fails_with_invalid_candidate_signature() {
		new_test_ext().execute_with(|| {
			set_attestation_allowance(ATTESTER, 5);

			assert_noop!(
				DotnsGateway::reserve_name(
					RuntimeOrigin::signed(ATTESTER),
					ALICE,
					invalid_candidate_signature(),
					base_name(ALICE_LITE),
					default_chat_key(),
					None,
					SIGNED_AT_NOW,
				),
				Error::<Test>::InvalidAttestationSignature
			);
		});
	}

	#[test]
	fn fails_with_empty_labels() {
		new_test_ext().execute_with(|| {
			set_attestation_allowance(ATTESTER, 5);

			// Empty lite label.
			assert_noop!(
				DotnsGateway::reserve_name(
					RuntimeOrigin::signed(ATTESTER),
					ALICE,
					valid_candidate_signature(ALICE),
					base_name(b""),
					default_chat_key(),
					None,
					SIGNED_AT_NOW,
				),
				Error::<Test>::InvalidName
			);

			// Empty reserved base label.
			assert_noop!(
				DotnsGateway::reserve_name(
					RuntimeOrigin::signed(ATTESTER),
					ALICE,
					valid_candidate_signature(ALICE),
					base_name(ALICE_LITE),
					default_chat_key(),
					Some(base_name(b"")),
					SIGNED_AT_NOW,
				),
				Error::<Test>::InvalidName
			);
		});
	}

	#[test]
	fn rejects_labels_outside_contract_format() {
		new_test_ext().execute_with(|| {
			set_attestation_allowance(ATTESTER, 20);

			// Lite-label position rejects single-DNS labels and malformed inputs.
			// A bare DNS label (`alice`) is valid for base but NOT for lite.
			for bad in [
				b"alice".to_vec(),
				b"Alice.42".to_vec(),
				b"alice.1".to_vec(),
				b"alice.".to_vec(),
				b".42".to_vec(),
				b"alice..42".to_vec(),
				b"a.b.42".to_vec(),
				b"-alice.42".to_vec(),
				b"alice-.42".to_vec(),
				vec![0xC3, 0xA9],
				vec![0xFF],
			] {
				assert_noop!(
					DotnsGateway::reserve_name(
						RuntimeOrigin::signed(ATTESTER),
						ALICE,
						valid_candidate_signature(ALICE),
						base_name(&bad),
						default_chat_key(),
						None,
						SIGNED_AT_NOW,
					),
					Error::<Test>::InvalidName
				);
			}

			// Reserved-base-label position rejects lite-format labels and malformed inputs.
			// A lite label (`alice.42`) is valid for lite but NOT for base.
			for bad in [
				ALICE_LITE.to_vec(),
				b"Alice".to_vec(),
				b"-alice".to_vec(),
				b"alice-".to_vec(),
				b"al.ice".to_vec(),
				vec![0xC3, 0xA9],
				vec![0xFF],
			] {
				assert_noop!(
					DotnsGateway::reserve_name(
						RuntimeOrigin::signed(ATTESTER),
						ALICE,
						valid_candidate_signature(ALICE),
						base_name(ALICE_LITE),
						default_chat_key(),
						Some(base_name(&bad)),
						SIGNED_AT_NOW,
					),
					Error::<Test>::InvalidName
				);
			}
		});
	}

	#[test]
	fn fails_when_contract_call_fails_without_revert_data() {
		new_test_ext().execute_with(|| {
			set_attestation_allowance(ATTESTER, 5);
			set_contract_call_dispatch_error(DispatchError::Other("oog"));

			assert_noop!(
				DotnsGateway::reserve_name(
					RuntimeOrigin::signed(ATTESTER),
					ALICE,
					valid_candidate_signature(ALICE),
					base_name(ALICE_LITE),
					default_chat_key(),
					None,
					SIGNED_AT_NOW,
				),
				Error::<Test>::ContractCallFailed
			);

			// Allowance not decremented on failure (tx reverted).
			assert_eq!(AttestationAllowance::<Test>::get(ATTESTER), 5);
		});
	}

	#[test]
	fn unknown_revert_data_collapses_to_contract_call_failed() {
		new_test_ext().execute_with(|| {
			set_attestation_allowance(ATTESTER, 5);
			set_contract_call_revert(vec![0xde, 0xad, 0xbe, 0xef]);

			assert_noop!(
				DotnsGateway::reserve_name(
					RuntimeOrigin::signed(ATTESTER),
					ALICE,
					valid_candidate_signature(ALICE),
					base_name(ALICE_LITE),
					default_chat_key(),
					None,
					SIGNED_AT_NOW,
				),
				Error::<Test>::ContractCallFailed
			);
		});
	}

	#[test]
	fn multiple_reservations_by_same_attester() {
		new_test_ext().execute_with(|| {
			set_attestation_allowance(ATTESTER, 2);

			// Reserving for Alice.
			assert_ok!(DotnsGateway::reserve_name(
				RuntimeOrigin::signed(ATTESTER),
				ALICE,
				valid_candidate_signature(ALICE),
				base_name(ALICE_LITE),
				default_chat_key(),
				None,
				SIGNED_AT_NOW,
			));

			// Reserving for Bob.
			assert_ok!(DotnsGateway::reserve_name(
				RuntimeOrigin::signed(ATTESTER),
				BOB,
				valid_candidate_signature(BOB),
				base_name(BOB_LITE),
				default_chat_key(),
				None,
				SIGNED_AT_NOW,
			));

			assert_eq!(get_contract_calls().len(), 2);
			assert!(!AttestationAllowance::<Test>::contains_key(ATTESTER));
		});
	}

	#[test]
	fn actual_weight_includes_contract_call_weight() {
		new_test_ext().execute_with(|| {
			set_attestation_allowance(ATTESTER, 1);
			let contract_weight = frame_support::weights::Weight::from_parts(200_000, 5_000);
			set_contract_call_weight(contract_weight);

			let result = DotnsGateway::reserve_name(
				RuntimeOrigin::signed(ATTESTER),
				ALICE,
				valid_candidate_signature(ALICE),
				base_name(ALICE_LITE),
				default_chat_key(),
				None,
				SIGNED_AT_NOW,
			);

			assert_ok!(&result);
			let info = result.unwrap();

			// Pallet overhead from default WeightInfo.
			let overhead = <Test as crate::Config>::WeightInfo::reserve_name();
			let expected = overhead.saturating_add(contract_weight);
			assert_eq!(info.actual_weight, Some(expected));
		});
	}

	#[test]
	fn rejects_expired_signature() {
		// `signed_at` is `MaxValiditySeconds + 1` seconds before "now"
		// (600+1=601s back), one second past the expiration boundary.
		new_test_ext().execute_with(|| {
			set_attestation_allowance(ATTESTER, 5);
			assert_noop!(
				DotnsGateway::reserve_name(
					RuntimeOrigin::signed(ATTESTER),
					ALICE,
					valid_candidate_signature(ALICE),
					base_name(ALICE_LITE),
					default_chat_key(),
					None,
					MOCK_NOW_BASE - 601,
				),
				Error::<Test>::ReservationSignatureExpired
			);
		});
	}

	#[test]
	fn accepts_signature_at_past_window_boundary() {
		// `signed_at == now - MaxValiditySeconds` is still within window
		new_test_ext().execute_with(|| {
			set_attestation_allowance(ATTESTER, 1);
			assert_ok!(DotnsGateway::reserve_name(
				RuntimeOrigin::signed(ATTESTER),
				ALICE,
				valid_candidate_signature(ALICE),
				base_name(ALICE_LITE),
				default_chat_key(),
				None,
				MOCK_NOW_BASE - 600,
			));
		});
	}

	#[test]
	fn rejects_future_dated_signature() {
		// `signed_at` is `MaxFutureSkewSeconds + 1` seconds ahead of "now"
		new_test_ext().execute_with(|| {
			set_attestation_allowance(ATTESTER, 5);
			assert_noop!(
				DotnsGateway::reserve_name(
					RuntimeOrigin::signed(ATTESTER),
					ALICE,
					valid_candidate_signature(ALICE),
					base_name(ALICE_LITE),
					default_chat_key(),
					None,
					MOCK_NOW_BASE + 16,
				),
				Error::<Test>::ReservationSignatureFromFuture
			);
		});
	}

	#[test]
	fn accepts_signature_at_future_window_boundary() {
		// `signed_at == now + MaxFutureSkewSeconds`
		new_test_ext().execute_with(|| {
			set_attestation_allowance(ATTESTER, 1);
			assert_ok!(DotnsGateway::reserve_name(
				RuntimeOrigin::signed(ATTESTER),
				ALICE,
				valid_candidate_signature(ALICE),
				base_name(ALICE_LITE),
				default_chat_key(),
				None,
				MOCK_NOW_BASE + 15,
			));
		});
	}

	#[test]
	fn reservation_with_reserved_label_succeeds() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			set_attestation_allowance(ATTESTER, 1);

			let result = DotnsGateway::reserve_name(
				RuntimeOrigin::signed(ATTESTER),
				ALICE,
				valid_candidate_signature(ALICE),
				base_name(ALICE_LITE),
				default_chat_key(),
				Some(base_name(ALICE_FULL)),
				SIGNED_AT_NOW,
			);

			assert_ok!(&result);

			// Single contract call dispatched.
			let calls = get_contract_calls();
			assert_eq!(calls.len(), 1);

			System::assert_last_event(
				crate::Event::<Test>::NameReserved {
					candidate: ALICE,
					attester: ATTESTER,
					lite_label: base_name(ALICE_LITE),
					chat_key: default_chat_key(),
					reserved_base_label: Some(base_name(ALICE_FULL)),
				}
				.into(),
			);

			// Reservation is free for the caller.
			let info = result.unwrap();
			assert_eq!(info.pays_fee, frame_support::dispatch::Pays::No);
		});
	}
}

mod registration {
	use super::*;
	use crate::extension::CustomValidity;
	use frame_support::dispatch::GetDispatchInfo;
	use sp_runtime::{
		traits::DispatchTransaction,
		transaction_validity::{InvalidTransaction, TransactionValidityError},
	};

	fn register_msg(who: u64, bn: &BaseLabel, link: &Link) -> Vec<u8> {
		DotnsGateway::construct_register_proof_message(&who, bn.as_slice(), link).to_vec()
	}

	fn assert_invalid_custom(
		result: &Result<
			(sp_runtime::transaction_validity::ValidTransaction, (), RuntimeOrigin),
			TransactionValidityError,
		>,
		expected: CustomValidity,
	) {
		match result {
			Err(TransactionValidityError::Invalid(InvalidTransaction::Custom(code))) => {
				assert_eq!(*code, expected as u8, "wrong custom code")
			},
			other => panic!("expected Custom({expected:?}), got {other:?}"),
		}
	}

	#[test]
	fn username_registration_with_lite_link_succeeds() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			LiteLabelOwner::<Test>::insert(base_name(ALICE_LITE), ALICE);
			AccountNames::<Test>::insert(
				ALICE,
				AccountNameRecord { lite: entry(ALICE_LITE, Some(lite_chat_key())), full: None },
			);
			let link = Link::LiteUsername(base_name(ALICE_LITE));
			let bn = base_name(ALICE_BASE);

			let result = DotnsGateway::register_name(
				person_registration_origin(alias_a()),
				ALICE,
				bn,
				link.clone(),
			);

			assert_ok!(&result);

			// Registration record stored.
			let record = AliasRegistration::<Test>::get(alias_a()).expect("record exists");
			assert_eq!(record.collection, Collection::People);
			assert_eq!(record.account, ALICE);

			// Full label added with the linked lite label's chat key; lite entry untouched.
			assert_eq!(
				AccountNames::<Test>::get(ALICE),
				Some(AccountNameRecord {
					lite: entry(ALICE_LITE, Some(lite_chat_key())),
					full: entry(ALICE_BASE, Some(lite_chat_key()))
				})
			);

			// Contract call dispatched to dispatcher address.
			let calls = get_contract_calls();
			assert_eq!(calls.len(), 1);
			assert_eq!(calls[0].0, DispatcherAddr::get());

			System::assert_last_event(
				crate::Event::<Test>::NameRegistered {
					alias: alias_a(),
					account: ALICE,
					label: base_name(ALICE_BASE),
					link,
				}
				.into(),
			);

			// Registration is free for the caller.
			let info = result.unwrap();
			assert_eq!(info.pays_fee, frame_support::dispatch::Pays::No);
		});
	}

	#[test]
	fn username_registration_standalone_succeeds() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			let link = Link::None(default_chat_key());
			let bn = base_name(ALICE_BASE);

			let result = DotnsGateway::register_name(
				person_registration_origin(alias_a()),
				ALICE,
				bn,
				link.clone(),
			);

			assert_ok!(&result);

			let record = AliasRegistration::<Test>::get(alias_a()).expect("record exists");
			assert_eq!(record.account, ALICE);
			assert_eq!(AccountAlias::<Test>::get(ALICE), Some(alias_a()));
			assert_eq!(
				AccountNames::<Test>::get(ALICE),
				Some(AccountNameRecord {
					lite: None,
					full: entry(ALICE_BASE, Some(default_chat_key()))
				})
			);

			System::assert_last_event(
				crate::Event::<Test>::NameRegistered {
					alias: alias_a(),
					account: ALICE,
					label: base_name(ALICE_BASE),
					link,
				}
				.into(),
			);

			let info = result.unwrap();
			assert_eq!(info.pays_fee, frame_support::dispatch::Pays::No);
		});
	}

	#[test]
	fn full_label_linked_to_an_unrecorded_lite_label_has_no_chat_key() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			LiteLabelOwner::<Test>::insert(base_name(ALICE_LITE), ALICE);
			// The account owns two lite labels and the record holds the most recent one. The
			// registration links the other label, whose key the pallet never recorded, so the
			// key the contracts copy to the full label is not known here.
			AccountNames::<Test>::insert(
				ALICE,
				AccountNameRecord { lite: entry(BOB_LITE, Some(lite_chat_key())), full: None },
			);

			assert_ok!(DotnsGateway::register_name(
				person_registration_origin(alias_a()),
				ALICE,
				base_name(ALICE_BASE),
				Link::LiteUsername(base_name(ALICE_LITE))
			));

			assert_eq!(
				AccountNames::<Test>::get(ALICE),
				Some(AccountNameRecord {
					lite: entry(BOB_LITE, Some(lite_chat_key())),
					full: entry(ALICE_BASE, None)
				})
			);
		});
	}

	#[test]
	fn standalone_registration_does_not_take_the_lite_chat_key() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			// `Link::None` registers a fresh key instead of linking, so the full entry gets that
			// key and the lite entry keeps its own.
			AccountNames::<Test>::insert(
				ALICE,
				AccountNameRecord { lite: entry(ALICE_LITE, Some(lite_chat_key())), full: None },
			);

			assert_ok!(DotnsGateway::register_name(
				person_registration_origin(alias_a()),
				ALICE,
				base_name(ALICE_BASE),
				Link::None(default_chat_key())
			));

			assert_eq!(
				AccountNames::<Test>::get(ALICE),
				Some(AccountNameRecord {
					lite: entry(ALICE_LITE, Some(lite_chat_key())),
					full: entry(ALICE_BASE, Some(default_chat_key()))
				})
			);
		});
	}

	#[test]
	fn fails_when_already_registered() {
		new_test_ext().execute_with(|| {
			let link = Link::None(default_chat_key());
			let bn1 = base_name(ALICE_BASE);
			let bn2 = base_name(BOB_BASE);

			// First registration succeeded.
			let (_, _, origin) = validate_register(
				valid_proof(alias_a(), &register_msg(ALICE, &bn1, &link)),
				0,
				people_revision(),
				offchain_signature(ALICE),
				ALICE,
				bn1.clone(),
				link.clone(),
			)
			.expect("validates");
			assert_ok!(DotnsGateway::register_name(origin, ALICE, bn1, link.clone()));

			// Same alias tries to register again.
			let result = validate_register(
				valid_proof(alias_a(), &register_msg(BOB, &bn2, &link)),
				0,
				people_revision(),
				offchain_signature(BOB),
				BOB,
				bn2,
				link,
			);
			assert_invalid_custom(&result, CustomValidity::AliasAlreadyRegistered);
		});
	}

	#[test]
	fn fails_with_invalid_proof() {
		new_test_ext().execute_with(|| {
			let link = Link::None(default_chat_key());
			let bn = base_name(ALICE_BASE);

			let result = validate_register(
				invalid_proof(),
				0,
				people_revision(),
				offchain_signature(ALICE),
				ALICE,
				bn,
				link,
			);

			assert!(matches!(
				result,
				Err(TransactionValidityError::Invalid(InvalidTransaction::BadProof)),
			));
		});
	}

	#[test]
	fn fails_with_empty_label() {
		new_test_ext().execute_with(|| {
			// Validation fires before proof check; any proof shape works.
			let result = validate_register(
				invalid_proof(),
				0,
				people_revision(),
				offchain_signature(ALICE),
				ALICE,
				base_name(b""),
				Link::None(default_chat_key()),
			);

			assert_invalid_custom(&result, CustomValidity::InvalidName);
		});
	}

	#[test]
	fn rejects_labels_outside_contract_format() {
		new_test_ext().execute_with(|| {
			// Base-label position rejects lite-format labels and malformed inputs.
			for bad in [
				ALICE_LITE.to_vec(),
				b"Alice".to_vec(),
				b"al.ice".to_vec(),
				b"-alice".to_vec(),
				b"alice-".to_vec(),
				vec![0xC3, 0xA9],
				vec![0xFF],
			] {
				let result = validate_register(
					invalid_proof(),
					0,
					people_revision(),
					offchain_signature(ALICE),
					ALICE,
					base_name(&bad),
					Link::None(default_chat_key()),
				);

				assert_invalid_custom(&result, CustomValidity::InvalidName);
			}

			// Link::LiteUsername payload rejects single-DNS labels and malformed inputs.
			for bad in [
				b"alice".to_vec(),
				b"Alice.42".to_vec(),
				b"alice.1".to_vec(),
				b"alice.".to_vec(),
				b".42".to_vec(),
				b"alice..42".to_vec(),
				vec![0xC3, 0xA9],
				vec![0xFF],
			] {
				let result = validate_register(
					invalid_proof(),
					0,
					people_revision(),
					offchain_signature(ALICE),
					ALICE,
					base_name(ALICE_BASE),
					Link::LiteUsername(base_name(&bad)),
				);

				assert_invalid_custom(&result, CustomValidity::InvalidName);
			}
		});
	}

	#[test]
	fn fails_when_contract_call_fails_without_revert_data() {
		new_test_ext().execute_with(|| {
			set_contract_call_dispatch_error(DispatchError::Other("oog"));

			let link = Link::None(default_chat_key());
			let bn = base_name(ALICE_BASE);

			assert_noop!(
				DotnsGateway::register_name(person_registration_origin(alias_a()), ALICE, bn, link,),
				Error::<Test>::ContractCallFailed
			);
		});
	}

	#[test]
	fn decodes_contract_revert_to_typed_error() {
		use alloy_core::sol_types::SolError;

		new_test_ext().execute_with(|| {
			set_contract_call_revert(crate::types::NotGateway::SELECTOR.to_vec());

			let link = Link::None(default_chat_key());
			let bn = base_name(ALICE_BASE);

			assert_noop!(
				DotnsGateway::register_name(person_registration_origin(alias_a()), ALICE, bn, link,),
				Error::<Test>::ContractRevert(DispatcherRevert::NotGateway)
			);
		});
	}

	#[test]
	fn decodes_dispatcher_not_root_revert() {
		use alloy_core::sol_types::SolError;

		new_test_ext().execute_with(|| {
			set_contract_call_revert(crate::types::NotRoot::SELECTOR.to_vec());

			let link = Link::None(default_chat_key());
			let bn = base_name(ALICE_BASE);

			assert_noop!(
				DotnsGateway::register_name(person_registration_origin(alias_a()), ALICE, bn, link),
				Error::<Test>::ContractRevert(DispatcherRevert::NotRoot)
			);
		});
	}

	#[test]
	fn proof_message_binding_rejects_mismatched_who() {
		new_test_ext().execute_with(|| {
			let link = Link::None(default_chat_key());
			let bn = base_name(ALICE_BASE);
			// Proof bound to (BOB, "alice", None) but call is from ALICE.
			let proof = valid_proof(alias_a(), &register_msg(BOB, &bn, &link));

			let result = validate_register(
				proof,
				0,
				people_revision(),
				offchain_signature(ALICE),
				ALICE,
				bn,
				link,
			);

			assert!(matches!(
				result,
				Err(TransactionValidityError::Invalid(InvalidTransaction::BadProof)),
			));
		});
	}

	#[test]
	fn proof_message_binding_rejects_mismatched_label() {
		new_test_ext().execute_with(|| {
			let link = Link::None(default_chat_key());
			let proof = valid_proof(alias_a(), &register_msg(ALICE, &base_name(ALICE_BASE), &link));

			let result = validate_register(
				proof,
				0,
				people_revision(),
				offchain_signature(ALICE),
				ALICE,
				base_name(BOB_BASE),
				link,
			);

			assert!(matches!(
				result,
				Err(TransactionValidityError::Invalid(InvalidTransaction::BadProof)),
			));
		});
	}

	#[test]
	fn proof_message_binding_rejects_mismatched_link() {
		new_test_ext().execute_with(|| {
			LiteLabelOwner::<Test>::insert(base_name(ALICE_LITE), ALICE);

			// Proof bound to Link::None but the call uses Link::LiteUsername.
			let bn = base_name(ALICE_BASE);
			let proof =
				valid_proof(alias_a(), &register_msg(ALICE, &bn, &Link::None(default_chat_key())));

			let result = validate_register(
				proof,
				0,
				people_revision(),
				offchain_signature(ALICE),
				ALICE,
				bn,
				Link::LiteUsername(base_name(ALICE_LITE)),
			);

			assert!(matches!(
				result,
				Err(TransactionValidityError::Invalid(InvalidTransaction::BadProof)),
			));
		});
	}

	#[test]
	fn fails_when_user_not_owns_specified_lite_name() {
		new_test_ext().execute_with(|| {
			LiteLabelOwner::<Test>::insert(base_name(ALICE_LITE), ALICE);

			let link = Link::LiteUsername(base_name(ALICE_LITE));
			let bn = base_name(BOB_BASE);
			let proof = valid_proof(alias_a(), &register_msg(BOB, &bn, &link));

			let result = validate_register(
				proof,
				0,
				people_revision(),
				offchain_signature(BOB),
				BOB,
				bn,
				link,
			);

			assert_invalid_custom(&result, CustomValidity::NotLiteLabelOwner);
		});
	}

	#[test]
	fn fails_when_unknown_lite_name_provided() {
		new_test_ext().execute_with(|| {
			let link = Link::LiteUsername(base_name(ALICE_LITE));
			let bn = base_name(ALICE_BASE);
			let proof = valid_proof(alias_a(), &register_msg(ALICE, &bn, &link));

			let result = validate_register(
				proof,
				0,
				people_revision(),
				offchain_signature(ALICE),
				ALICE,
				bn,
				link,
			);

			assert_invalid_custom(&result, CustomValidity::NotLiteLabelOwner);
		});
	}

	#[test]
	fn reserve_then_register_by_same_user_succeeds() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			set_attestation_allowance(ATTESTER, 1);

			assert_ok!(DotnsGateway::reserve_name(
				RuntimeOrigin::signed(ATTESTER),
				ALICE,
				valid_candidate_signature(ALICE),
				base_name(ALICE_LITE),
				default_chat_key(),
				None,
				SIGNED_AT_NOW,
			));
			assert_eq!(LiteLabelOwner::<Test>::get(base_name(ALICE_LITE)), Some(ALICE));

			let link = Link::LiteUsername(base_name(ALICE_LITE));
			let bn = base_name(ALICE_BASE);
			let proof = valid_proof(alias_a(), &register_msg(ALICE, &bn, &link));

			let (_validity, _, origin) = validate_register(
				proof,
				0,
				people_revision(),
				offchain_signature(ALICE),
				ALICE,
				bn.clone(),
				link.clone(),
			)
			.expect("validates");

			assert_ok!(DotnsGateway::register_name(origin, ALICE, bn, link));
		});
	}

	#[test]
	fn actual_weight_includes_contract_call_weight() {
		new_test_ext().execute_with(|| {
			let contract_weight = frame_support::weights::Weight::from_parts(300_000, 8_000);
			set_contract_call_weight(contract_weight);

			let link = Link::None(default_chat_key());
			let bn = base_name(ALICE_BASE);

			let result =
				DotnsGateway::register_name(person_registration_origin(alias_a()), ALICE, bn, link);

			assert_ok!(&result);
			let info = result.unwrap();

			// Pallet overhead from default WeightInfo.
			let overhead = <Test as crate::Config>::WeightInfo::register_name();
			let expected = overhead.saturating_add(contract_weight);
			assert_eq!(info.actual_weight, Some(expected));
		});
	}

	#[test]
	fn rejects_signed_origin() {
		new_test_ext().execute_with(|| {
			let link = Link::None(default_chat_key());
			let bn = base_name(ALICE_BASE);

			assert_noop!(
				DotnsGateway::register_name(RuntimeOrigin::signed(ALICE), ALICE, bn, link),
				DispatchError::BadOrigin
			);
		});
	}

	#[test]
	fn rejects_root_origin() {
		new_test_ext().execute_with(|| {
			let link = Link::None(default_chat_key());
			let bn = base_name(ALICE_BASE);

			assert_noop!(
				DotnsGateway::register_name(RuntimeOrigin::root(), ALICE, bn, link),
				DispatchError::BadOrigin
			);
		});
	}

	#[test]
	fn rejects_none_origin() {
		new_test_ext().execute_with(|| {
			let link = Link::None(default_chat_key());
			let bn = base_name(ALICE_BASE);

			assert_noop!(
				DotnsGateway::register_name(RuntimeOrigin::none(), ALICE, bn, link),
				DispatchError::BadOrigin
			);
		});
	}

	#[test]
	fn valid_proof_and_signature_produce_person_registration_origin() {
		new_test_ext().execute_with(|| {
			let link = Link::None(default_chat_key());
			let bn = base_name(ALICE_BASE);
			let proof = valid_proof(alias_a(), &register_msg(ALICE, &bn, &link));
			let signature = offchain_signature(ALICE);

			let (_validity, _, origin) =
				validate_register(proof, 0, people_revision(), signature, ALICE, bn, link)
					.expect("validates");

			// The extension must mutate the None origin into PersonRegistration.
			assert_eq!(
				DotnsGateway::ensure_person_registration(origin).expect("custom origin"),
				alias_a(),
			);
		});
	}

	#[test]
	fn rejects_non_none_origin() {
		new_test_ext().execute_with(|| {
			let link = Link::None(default_chat_key());
			let bn = base_name(ALICE_BASE);
			let proof = valid_proof(alias_a(), &register_msg(ALICE, &bn, &link));

			let tx_ext = crate::AsDotnsGateway::<Test>::new(Some(
				crate::AsDotnsGatewayInfo::RegisterFullName {
					proof,
					ring_index: 0,
					revision: people_revision(),
					signature: offchain_signature(ALICE),
				},
			));
			let call = RuntimeCall::DotnsGateway(crate::pallet::Call::register_name {
				who: ALICE,
				label: bn,
				link,
			});
			let info = call.get_dispatch_info();

			let result = tx_ext.validate_only(
				RuntimeOrigin::signed(ALICE),
				&call,
				&info,
				0,
				TransactionSource::External,
				0,
			);

			assert_invalid_custom(&result, CustomValidity::OriginNotNone);
		});
	}

	#[test]
	fn rejects_wrong_call() {
		// Even from a None origin, the extension should refuse to operate on a
		// call other than `register_name`.
		new_test_ext().execute_with(|| {
			let tx_ext = crate::AsDotnsGateway::<Test>::new(Some(
				crate::AsDotnsGatewayInfo::RegisterFullName {
					proof: invalid_proof(),
					ring_index: 0,
					revision: people_revision(),
					signature: offchain_signature(ALICE),
				},
			));
			let call = RuntimeCall::DotnsGateway(crate::pallet::Call::set_dispatcher_address {
				address: H160([0x99; 20]),
			});
			let info = call.get_dispatch_info();

			let result = tx_ext.validate_only(
				RuntimeOrigin::none(),
				&call,
				&info,
				0,
				TransactionSource::External,
				0,
			);

			assert!(matches!(
				result,
				Err(TransactionValidityError::Invalid(InvalidTransaction::Call)),
			));
		});
	}

	#[test]
	fn rejects_invalid_offchain_signature() {
		new_test_ext().execute_with(|| {
			let link = Link::None(default_chat_key());
			let bn = base_name(ALICE_BASE);
			let proof = valid_proof(alias_a(), &register_msg(ALICE, &bn, &link));

			let result = validate_register(
				proof,
				0,
				people_revision(),
				invalid_offchain_signature(),
				ALICE,
				bn,
				link,
			);

			assert_invalid_custom(&result, CustomValidity::InvalidOffchainSignature);
		});
	}

	#[test]
	fn rejects_when_account_already_registered() {
		new_test_ext().execute_with(|| {
			let link = Link::None(default_chat_key());
			let bn = base_name(ALICE_BASE);

			// Pre-populate AccountAlias for ALICE with a *different* alias to
			// simulate Alice already having a registration. A fresh proof from
			// another member targeting `who=ALICE` must be rejected.
			AccountAlias::<Test>::insert(ALICE, [9u8; 32]);

			let other_alias = [2u8; 32];
			let proof = valid_proof(other_alias, &register_msg(ALICE, &bn, &link));

			let result = validate_register(
				proof,
				0,
				people_revision(),
				offchain_signature(ALICE),
				ALICE,
				bn,
				link,
			);

			assert_invalid_custom(&result, CustomValidity::AccountAlreadyRegistered);
		});
	}

	#[test]
	fn end_to_end_extension_then_dispatch() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			let link = Link::None(default_chat_key());
			let bn = base_name(ALICE_BASE);
			let proof = valid_proof(alias_a(), &register_msg(ALICE, &bn, &link));

			let (_validity, _, origin) = validate_register(
				proof,
				0,
				people_revision(),
				offchain_signature(ALICE),
				ALICE,
				bn.clone(),
				link.clone(),
			)
			.expect("validates");

			assert_ok!(DotnsGateway::register_name(origin, ALICE, bn, link));

			assert_eq!(AccountAlias::<Test>::get(ALICE), Some(alias_a()));
		});
	}
}

mod signed_message {
	use super::*;

	fn build(
		candidate: u64,
		attester: u64,
		username_base: &[u8],
		chat: &[u8; 65],
		reserved: Option<&[u8]>,
		signed_at: u64,
	) -> Vec<u8> {
		DotnsGateway::construct_reservation_message(
			&candidate,
			&attester,
			username_base,
			chat,
			reserved,
			signed_at,
		)
	}

	#[test]
	fn distinct_bases_produce_distinct_messages() {
		let key = [0xABu8; 65];
		assert_ne!(build(1, 10, b"abc", &key, None, 0), build(1, 10, b"abd", &key, None, 0));
	}

	#[test]
	fn same_base_different_digits_produces_equal_messages_via_lite_base() {
		let key = [0xABu8; 65];
		let lite_42 = base_name(b"alice.42");
		let lite_43 = base_name(b"alice.43");

		assert_eq!(
			build(1, 10, lite_42.lite_base(), &key, None, 0),
			build(1, 10, lite_43.lite_base(), &key, None, 0),
		);
	}

	#[test]
	fn reserved_label_some_vs_none_produces_distinct_messages() {
		let key = [0xABu8; 65];
		assert_ne!(
			build(1, 10, ALICE_BASE, &key, None, 0),
			build(1, 10, ALICE_BASE, &key, Some(ALICE_BASE), 0),
		);
	}

	#[test]
	fn boundary_shift_between_base_and_reserved_is_unambiguous() {
		let key = [0xABu8; 65];
		assert_ne!(build(1, 10, b"abcd", &key, None, 0), build(1, 10, b"abc", &key, Some(b"d"), 0),);
	}

	#[test]
	fn distinct_candidate_or_attester_produces_distinct_messages() {
		let key = [0xABu8; 65];
		assert_ne!(
			build(1, 10, ALICE_BASE, &key, None, 0),
			build(2, 10, ALICE_BASE, &key, None, 0),
		);
		assert_ne!(
			build(1, 10, ALICE_BASE, &key, None, 0),
			build(1, 11, ALICE_BASE, &key, None, 0),
		);
	}

	#[test]
	fn distinct_signed_at_produce_distinct_messages() {
		let key = [0xABu8; 65];
		assert_ne!(
			build(1, 10, ALICE_BASE, &key, None, 100),
			build(1, 10, ALICE_BASE, &key, None, 101),
		);
	}

	#[test]
	fn equal_inputs_produce_equal_messages() {
		let key = [0xABu8; 65];
		assert_eq!(
			build(1, 10, ALICE_BASE, &key, Some(ALICE_FULL), 42),
			build(1, 10, ALICE_BASE, &key, Some(ALICE_FULL), 42),
		);
	}
}

mod migration {
	use super::*;
	use codec::Encode;
	use frame_support::storage::unhashed;

	/// Stores a record for `who` in the two-field shape used before the chat key existed.
	fn seed_old_record(who: u64, lite: Option<&[u8]>, full: Option<&[u8]>) {
		let value = (lite.map(base_name), full.map(base_name)).encode();
		unhashed::put_raw(&AccountNames::<Test>::hashed_key_for(who), &value);
	}

	#[test]
	fn v1_backfills_lite_labels_from_lite_label_owner() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<DotnsGateway>();
			LiteLabelOwner::<Test>::insert(base_name(ALICE_LITE), ALICE);
			LiteLabelOwner::<Test>::insert(base_name(BOB_LITE), BOB);
			// An existing two-field record keeps its fields and gains the label.
			seed_old_record(BOB, None, Some(BOB_BASE));
			// A two-field record does not decode under the current type until migrated.
			assert_eq!(AccountNames::<Test>::get(BOB), None);

			MigrateV0ToV1::<Test>::on_runtime_upgrade();

			assert_eq!(
				AccountNames::<Test>::get(ALICE),
				Some(AccountNameRecord { lite: entry(ALICE_LITE, None), full: None })
			);
			assert_eq!(
				AccountNames::<Test>::get(BOB),
				Some(AccountNameRecord {
					lite: entry(BOB_LITE, None),
					full: entry(BOB_BASE, None)
				})
			);
			assert_eq!(DotnsGateway::on_chain_storage_version(), StorageVersion::new(1));
		});
	}

	#[test]
	fn v1_keeps_an_existing_lite_label() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(0).put::<DotnsGateway>();
			LiteLabelOwner::<Test>::insert(base_name(ALICE_LITE), ALICE);
			LiteLabelOwner::<Test>::insert(base_name(BOB_LITE), ALICE);
			seed_old_record(ALICE, Some(BOB_LITE), None);

			MigrateV0ToV1::<Test>::on_runtime_upgrade();

			assert_eq!(
				AccountNames::<Test>::get(ALICE),
				Some(AccountNameRecord { lite: entry(BOB_LITE, None), full: None })
			);
		});
	}

	#[test]
	fn v1_runs_only_from_version_zero() {
		new_test_ext().execute_with(|| {
			StorageVersion::new(1).put::<DotnsGateway>();
			LiteLabelOwner::<Test>::insert(base_name(ALICE_LITE), ALICE);

			MigrateV0ToV1::<Test>::on_runtime_upgrade();

			assert_eq!(AccountNames::<Test>::get(ALICE), None);
		});
	}
}
