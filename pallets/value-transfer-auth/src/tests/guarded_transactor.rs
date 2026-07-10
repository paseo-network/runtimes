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
	allow_only_siblings::AllowOnlySiblings, extension::block_flag, ProtectedAssetTransactor,
};
use frame_support::{
	parameter_types,
	traits::{
		tokens::imbalance::{
			ImbalanceAccounting, UnsafeConstructorDestructor, UnsafeManualAccounting,
		},
		Contains,
	},
};
use std::sync::{
	atomic::{AtomicUsize, Ordering},
	Mutex,
};
use xcm::latest::{
	Asset, AssetId, Error as XcmError, Fungibility, Junction::Parachain, Location, XcmContext,
	XcmHash,
};
use xcm_executor::{traits::TransactAsset, AssetsInHolding};

static TEST_LOCK: Mutex<()> = Mutex::new(());
static DEPOSIT_CALLS: AtomicUsize = AtomicUsize::new(0);
static WITHDRAW_CALLS: AtomicUsize = AtomicUsize::new(0);
static INTERNAL_TRANSFER_CALLS: AtomicUsize = AtomicUsize::new(0);
static MINT_CALLS: AtomicUsize = AtomicUsize::new(0);
static CAN_CHECK_IN_CALLS: AtomicUsize = AtomicUsize::new(0);
static CHECK_IN_CALLS: AtomicUsize = AtomicUsize::new(0);
static CAN_CHECK_OUT_CALLS: AtomicUsize = AtomicUsize::new(0);
static CHECK_OUT_CALLS: AtomicUsize = AtomicUsize::new(0);

parameter_types! {
	pub ProtectedAssetLocation: Location = Location::new(1, [Parachain(1500)]);
	pub NextAhId: u32 = 1500;
	pub NextPeopleId: u32 = 1502;
}

struct TrustedNone;
impl Contains<Location> for TrustedNone {
	fn contains(_: &Location) -> bool {
		false
	}
}

type TrustedNextAh = AllowOnlySiblings<NextAhId, NextPeopleId>;

type Guard = ProtectedAssetTransactor<Inner, ProtectedAssetLocation, TrustedNone>;
type GuardWithTrusted = ProtectedAssetTransactor<Inner, ProtectedAssetLocation, TrustedNextAh>;

struct Inner;

impl TransactAsset for Inner {
	fn can_check_in(
		_origin: &Location,
		_what: &Asset,
		_context: &XcmContext,
	) -> xcm::latest::Result {
		CAN_CHECK_IN_CALLS.fetch_add(1, Ordering::SeqCst);
		Ok(())
	}

	fn check_in(_origin: &Location, _what: &Asset, _context: &XcmContext) {
		CHECK_IN_CALLS.fetch_add(1, Ordering::SeqCst);
	}

	fn can_check_out(
		_dest: &Location,
		_what: &Asset,
		_context: &XcmContext,
	) -> xcm::latest::Result {
		CAN_CHECK_OUT_CALLS.fetch_add(1, Ordering::SeqCst);
		Ok(())
	}

	fn check_out(_dest: &Location, _what: &Asset, _context: &XcmContext) {
		CHECK_OUT_CALLS.fetch_add(1, Ordering::SeqCst);
	}

	fn deposit_asset(
		what: AssetsInHolding,
		_who: &Location,
		_context: Option<&XcmContext>,
	) -> Result<(), (AssetsInHolding, XcmError)> {
		DEPOSIT_CALLS.fetch_add(1, Ordering::SeqCst);
		let _ = what;
		Ok(())
	}

	fn withdraw_asset(
		_what: &Asset,
		_who: &Location,
		_context: Option<&XcmContext>,
	) -> Result<AssetsInHolding, XcmError> {
		WITHDRAW_CALLS.fetch_add(1, Ordering::SeqCst);
		Ok(AssetsInHolding::new())
	}

	fn internal_transfer_asset(
		_what: &Asset,
		_from: &Location,
		_to: &Location,
		_context: &XcmContext,
	) -> Result<Asset, XcmError> {
		INTERNAL_TRANSFER_CALLS.fetch_add(1, Ordering::SeqCst);
		Ok(protected_asset())
	}

	fn mint_asset(_what: &Asset, _context: &XcmContext) -> Result<AssetsInHolding, XcmError> {
		MINT_CALLS.fetch_add(1, Ordering::SeqCst);
		Ok(AssetsInHolding::new())
	}
}

struct MockCredit(u128);

impl UnsafeConstructorDestructor<u128> for MockCredit {
	fn unsafe_clone(&self) -> Box<dyn ImbalanceAccounting<u128>> {
		Box::new(MockCredit(self.0))
	}

	fn forget_imbalance(&mut self) -> u128 {
		let amount = self.0;
		self.0 = 0;
		amount
	}
}

impl UnsafeManualAccounting<u128> for MockCredit {
	fn saturating_subsume(&mut self, mut other: Box<dyn ImbalanceAccounting<u128>>) {
		self.0 = self.0.saturating_add(other.forget_imbalance());
	}
}

impl ImbalanceAccounting<u128> for MockCredit {
	fn amount(&self) -> u128 {
		self.0
	}

	fn saturating_take(&mut self, amount: u128) -> Box<dyn ImbalanceAccounting<u128>> {
		let taken = self.0.min(amount);
		self.0 -= taken;
		Box::new(MockCredit(taken))
	}
}

fn reset() -> std::sync::MutexGuard<'static, ()> {
	let guard = TEST_LOCK.lock().expect("test lock is not poisoned");
	block_flag::block();
	DEPOSIT_CALLS.store(0, Ordering::SeqCst);
	WITHDRAW_CALLS.store(0, Ordering::SeqCst);
	INTERNAL_TRANSFER_CALLS.store(0, Ordering::SeqCst);
	MINT_CALLS.store(0, Ordering::SeqCst);
	CAN_CHECK_IN_CALLS.store(0, Ordering::SeqCst);
	CHECK_IN_CALLS.store(0, Ordering::SeqCst);
	CAN_CHECK_OUT_CALLS.store(0, Ordering::SeqCst);
	CHECK_OUT_CALLS.store(0, Ordering::SeqCst);
	guard
}

fn context() -> XcmContext {
	XcmContext { origin: None, message_id: XcmHash::default(), topic: None }
}

fn context_with_origin(origin: Location) -> XcmContext {
	XcmContext { origin: Some(origin), message_id: XcmHash::default(), topic: None }
}

fn protected_asset() -> Asset {
	Asset { id: AssetId(ProtectedAssetLocation::get()), fun: Fungibility::Fungible(100) }
}

fn non_protected_asset() -> Asset {
	Asset { id: AssetId(Location::new(1, [Parachain(3000)])), fun: Fungibility::Fungible(100) }
}

fn holding(asset: Asset) -> AssetsInHolding {
	AssetsInHolding::new_from_fungible_credit(asset.id, Box::new(MockCredit(100)))
}

#[test]
fn withdraw_protected_asset_when_blocked_rejects() {
	let _guard = reset();

	assert_eq!(
		Guard::withdraw_asset(&protected_asset(), &Location::here(), None),
		Err(XcmError::NoPermission)
	);
	assert_eq!(WITHDRAW_CALLS.load(Ordering::SeqCst), 0);
}

#[test]
fn withdraw_protected_asset_when_unblocked_passes_to_inner() {
	let _guard = reset();
	block_flag::unblock();

	assert!(Guard::withdraw_asset(&protected_asset(), &Location::here(), None).is_ok());
	assert_eq!(WITHDRAW_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn withdraw_non_protected_asset_passes_when_blocked() {
	let _guard = reset();

	assert!(Guard::withdraw_asset(&non_protected_asset(), &Location::here(), None).is_ok());
	assert_eq!(WITHDRAW_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn deposit_protected_asset_when_blocked_rejects() {
	let _guard = reset();

	let result = Guard::deposit_asset(holding(protected_asset()), &Location::here(), None);
	assert!(matches!(result, Err((_, XcmError::NoPermission))));
	assert_eq!(DEPOSIT_CALLS.load(Ordering::SeqCst), 0);
}

#[test]
fn deposit_protected_asset_when_unblocked_passes_to_inner() {
	let _guard = reset();
	block_flag::unblock();

	assert!(Guard::deposit_asset(holding(protected_asset()), &Location::here(), None).is_ok());
	assert_eq!(DEPOSIT_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn deposit_non_protected_asset_passes_when_blocked() {
	let _guard = reset();

	assert!(Guard::deposit_asset(holding(non_protected_asset()), &Location::here(), None).is_ok());
	assert_eq!(DEPOSIT_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn internal_transfer_protected_asset_when_blocked_rejects() {
	let _guard = reset();
	let context = context();

	assert_eq!(
		Guard::internal_transfer_asset(
			&protected_asset(),
			&Location::here(),
			&Location::parent(),
			&context
		),
		Err(XcmError::NoPermission)
	);
	assert_eq!(INTERNAL_TRANSFER_CALLS.load(Ordering::SeqCst), 0);
}

#[test]
fn internal_transfer_protected_asset_when_unblocked_passes_to_inner() {
	let _guard = reset();
	block_flag::unblock();
	let context = context();

	assert!(Guard::internal_transfer_asset(
		&protected_asset(),
		&Location::here(),
		&Location::parent(),
		&context
	)
	.is_ok());
	assert_eq!(INTERNAL_TRANSFER_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn internal_transfer_non_protected_asset_passes_when_blocked() {
	let _guard = reset();
	let context = context();

	assert!(Guard::internal_transfer_asset(
		&non_protected_asset(),
		&Location::here(),
		&Location::parent(),
		&context
	)
	.is_ok());
	assert_eq!(INTERNAL_TRANSFER_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn pass_through_check_in_check_out() {
	let _guard = reset();
	let context = context();

	assert!(Guard::can_check_in(&Location::parent(), &protected_asset(), &context).is_ok());
	Guard::check_in(&Location::parent(), &protected_asset(), &context);
	assert!(Guard::can_check_out(&Location::parent(), &protected_asset(), &context).is_ok());
	Guard::check_out(&Location::parent(), &protected_asset(), &context);

	assert_eq!(CAN_CHECK_IN_CALLS.load(Ordering::SeqCst), 1);
	assert_eq!(CHECK_IN_CALLS.load(Ordering::SeqCst), 1);
	assert_eq!(CAN_CHECK_OUT_CALLS.load(Ordering::SeqCst), 1);
	assert_eq!(CHECK_OUT_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn deposit_from_trusted_sibling_passes_when_blocked() {
	let _guard = reset();
	let ctx = context_with_origin(Location::new(1, [Parachain(1500)]));

	let result =
		GuardWithTrusted::deposit_asset(holding(protected_asset()), &Location::here(), Some(&ctx));
	assert!(result.is_ok());
	assert_eq!(DEPOSIT_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn deposit_from_untrusted_origin_rejected_when_blocked() {
	let _guard = reset();
	let ctx = context_with_origin(Location::new(1, [Parachain(9999)]));

	let result =
		GuardWithTrusted::deposit_asset(holding(protected_asset()), &Location::here(), Some(&ctx));
	assert!(matches!(result, Err((_, XcmError::NoPermission))));
	assert_eq!(DEPOSIT_CALLS.load(Ordering::SeqCst), 0);
}

#[test]
fn deposit_with_no_context_rejected_when_blocked() {
	let _guard = reset();

	let result =
		GuardWithTrusted::deposit_asset(holding(protected_asset()), &Location::here(), None);
	assert!(matches!(result, Err((_, XcmError::NoPermission))));
	assert_eq!(DEPOSIT_CALLS.load(Ordering::SeqCst), 0);
}

#[test]
fn mint_from_trusted_sibling_passes_when_blocked() {
	let _guard = reset();
	let ctx = context_with_origin(Location::new(1, [Parachain(1500)]));

	let result = GuardWithTrusted::mint_asset(&protected_asset(), &ctx);
	assert!(result.is_ok());
	assert_eq!(MINT_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn mint_from_untrusted_origin_rejected_when_blocked() {
	let _guard = reset();
	let ctx = context_with_origin(Location::new(1, [Parachain(9999)]));

	let result = GuardWithTrusted::mint_asset(&protected_asset(), &ctx);
	assert_eq!(result, Err(XcmError::NoPermission));
	assert_eq!(MINT_CALLS.load(Ordering::SeqCst), 0);
}
