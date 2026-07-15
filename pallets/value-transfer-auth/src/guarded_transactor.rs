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

//! `ProtectedAssetTransactor` is the XCM-side counterpart to `RestrictProtectedAssetErc20`.
//!
//! It rejects every operation on a protected-asset `Location` while the global
//! value-transfer block flag is set. Wrapping the XCM executor's asset transactor catches every XCM
//! path into pallet-assets at the trait level.
//! The `ProtectedAssetLocation` type parameter is runtime configuration, so the same wrapper can
//! guard any protected asset location selected by the runtime.

use crate::extension::block_flag;
use core::marker::PhantomData;
use frame_support::{
	traits::{Contains, Get},
	weights::Weight,
};
use xcm::latest::{Asset, Error as XcmError, Location, Result as XcmResult, XcmContext};
use xcm_executor::{traits::TransactAsset, AssetsInHolding};

pub struct ProtectedAssetTransactor<Inner, ProtectedAssetLocation, TrustedSiblings>(
	PhantomData<(Inner, ProtectedAssetLocation, TrustedSiblings)>,
);

impl<Inner, ProtectedAssetLocation, TrustedSiblings> TransactAsset
	for ProtectedAssetTransactor<Inner, ProtectedAssetLocation, TrustedSiblings>
where
	Inner: TransactAsset,
	ProtectedAssetLocation: Get<Location>,
	TrustedSiblings: Contains<Location>,
{
	fn can_check_in(origin: &Location, what: &Asset, context: &XcmContext) -> XcmResult {
		Inner::can_check_in(origin, what, context)
	}

	fn check_in(origin: &Location, what: &Asset, context: &XcmContext) {
		Inner::check_in(origin, what, context)
	}

	fn can_check_out(dest: &Location, what: &Asset, context: &XcmContext) -> XcmResult {
		Inner::can_check_out(dest, what, context)
	}

	fn check_out(dest: &Location, what: &Asset, context: &XcmContext) {
		Inner::check_out(dest, what, context)
	}

	fn deposit_asset(
		what: AssetsInHolding,
		who: &Location,
		context: Option<&XcmContext>,
	) -> Result<(), (AssetsInHolding, XcmError)> {
		if holding_contains_protected_asset::<ProtectedAssetLocation>(&what) &&
			block_flag::is_blocked()
		{
			// A cleared origin (`None`) is the teleport-settlement shape: `InitiateTeleport`
			// pushes `ClearOrigin` before `DepositAsset`, so by the time we deposit the origin
			// has been nulled. Provenance was already enforced one instruction earlier at
			// `ReceiveTeleportedAsset` by `IsTeleporter`/`ExternalAssetFromAssetHub`, so we permit
			// the settled inflow. A `Some(untrusted)` origin is still rejected.
			let from_trusted_origin = match context.and_then(|c| c.origin.as_ref()) {
				Some(o) => TrustedSiblings::contains(o),
				None => true,
			};
			if !from_trusted_origin {
				return Err((what, XcmError::NoPermission));
			}
		}

		Inner::deposit_asset(what, who, context)
	}

	fn deposit_asset_with_surplus(
		what: AssetsInHolding,
		who: &Location,
		context: Option<&XcmContext>,
	) -> Result<Weight, (AssetsInHolding, XcmError)> {
		if holding_contains_protected_asset::<ProtectedAssetLocation>(&what) &&
			block_flag::is_blocked()
		{
			// Mirror of `deposit_asset`: permit a cleared (`None`) origin, which is the
			// post-`ClearOrigin` teleport-settlement shape whose provenance was already checked at
			// `ReceiveTeleportedAsset`; still reject a `Some(untrusted)` origin.
			let from_trusted_origin = match context.and_then(|c| c.origin.as_ref()) {
				Some(o) => TrustedSiblings::contains(o),
				None => true,
			};
			if !from_trusted_origin {
				return Err((what, XcmError::NoPermission));
			}
		}

		Inner::deposit_asset_with_surplus(what, who, context)
	}

	fn withdraw_asset(
		what: &Asset,
		who: &Location,
		maybe_context: Option<&XcmContext>,
	) -> Result<AssetsInHolding, XcmError> {
		if is_protected_asset::<ProtectedAssetLocation>(what) && block_flag::is_blocked() {
			return Err(XcmError::NoPermission);
		}

		Inner::withdraw_asset(what, who, maybe_context)
	}

	fn withdraw_asset_with_surplus(
		what: &Asset,
		who: &Location,
		maybe_context: Option<&XcmContext>,
	) -> Result<(AssetsInHolding, Weight), XcmError> {
		if is_protected_asset::<ProtectedAssetLocation>(what) && block_flag::is_blocked() {
			return Err(XcmError::NoPermission);
		}

		Inner::withdraw_asset_with_surplus(what, who, maybe_context)
	}

	fn internal_transfer_asset(
		what: &Asset,
		from: &Location,
		to: &Location,
		context: &XcmContext,
	) -> Result<Asset, XcmError> {
		if is_protected_asset::<ProtectedAssetLocation>(what) && block_flag::is_blocked() {
			return Err(XcmError::NoPermission);
		}

		Inner::internal_transfer_asset(what, from, to, context)
	}

	fn internal_transfer_asset_with_surplus(
		what: &Asset,
		from: &Location,
		to: &Location,
		context: &XcmContext,
	) -> Result<(Asset, Weight), XcmError> {
		if is_protected_asset::<ProtectedAssetLocation>(what) && block_flag::is_blocked() {
			return Err(XcmError::NoPermission);
		}

		Inner::internal_transfer_asset_with_surplus(what, from, to, context)
	}

	fn transfer_asset(
		asset: &Asset,
		from: &Location,
		to: &Location,
		context: &XcmContext,
	) -> Result<Asset, XcmError> {
		if is_protected_asset::<ProtectedAssetLocation>(asset) && block_flag::is_blocked() {
			return Err(XcmError::NoPermission);
		}

		Inner::transfer_asset(asset, from, to, context)
	}

	fn transfer_asset_with_surplus(
		asset: &Asset,
		from: &Location,
		to: &Location,
		context: &XcmContext,
	) -> Result<(Asset, Weight), XcmError> {
		if is_protected_asset::<ProtectedAssetLocation>(asset) && block_flag::is_blocked() {
			return Err(XcmError::NoPermission);
		}

		Inner::transfer_asset_with_surplus(asset, from, to, context)
	}

	fn mint_asset(what: &Asset, context: &XcmContext) -> Result<AssetsInHolding, XcmError> {
		if is_protected_asset::<ProtectedAssetLocation>(what) && block_flag::is_blocked() {
			let from_trusted_origin =
				context.origin.as_ref().map(|o| TrustedSiblings::contains(o)).unwrap_or(false);
			if !from_trusted_origin {
				return Err(XcmError::NoPermission);
			}
		}

		Inner::mint_asset(what, context)
	}
}

fn holding_contains_protected_asset<W: Get<Location>>(assets: &AssetsInHolding) -> bool {
	assets.assets_iter().any(|asset| is_protected_asset::<W>(&asset))
}

fn is_protected_asset<W: Get<Location>>(asset: &Asset) -> bool {
	asset.id.0 == W::get()
}
