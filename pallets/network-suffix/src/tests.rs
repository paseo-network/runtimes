// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Individuality.
// SPDX-License-Identifier: Apache-2.0
//
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

use crate::{mock::*, Error, Event};
use frame_support::{assert_noop, assert_ok, traits::Get};
use frame_system::RawOrigin;

#[test]
fn genesis_config_sets_network_suffix() {
	new_test_ext_with_suffix(b"test").execute_with(|| {
		assert_eq!(NetworkSuffix::get().as_slice(), b"test");
	});
}

#[test]
fn default_genesis_uses_runtime_default() {
	new_test_ext().execute_with(|| {
		assert_eq!(NetworkSuffix::get().as_slice(), b"paseo");
	});
}

#[test]
fn root_can_override_network_suffix() {
	new_test_ext().execute_with(|| {
		assert_ok!(NetworkSuffix::set_network_suffix(
			RuntimeOrigin::root(),
			b"test".to_vec().try_into().unwrap()
		));
		assert_eq!(NetworkSuffix::get().as_slice(), b"test");
		System::assert_last_event(
			Event::NetworkSuffixSet {
				old: b"paseo".to_vec().try_into().unwrap(),
				new: b"test".to_vec().try_into().unwrap(),
			}
			.into(),
		);
	});
}

#[test]
fn signed_origin_cannot_override_network_suffix() {
	new_test_ext().execute_with(|| {
		assert_noop!(
			NetworkSuffix::set_network_suffix(
				RuntimeOrigin::signed(1),
				b"test".to_vec().try_into().unwrap()
			),
			sp_runtime::DispatchError::BadOrigin
		);
	});
}

#[test]
fn empty_suffix_is_rejected() {
	new_test_ext().execute_with(|| {
		assert_noop!(
			NetworkSuffix::set_network_suffix(
				RawOrigin::Root.into(),
				Vec::new().try_into().unwrap()
			),
			Error::<Test>::EmptySuffix
		);
	});
}
