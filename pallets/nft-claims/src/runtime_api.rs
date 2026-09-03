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

//! Runtime API definition for previewing NFT claim selections.

use alloc::vec::Vec;
use codec::{Decode, DecodeWithMemTracking, Encode};
use indiv_support::credit_trees::NftClaimCredit;
use pallet_scarcity::{CollectionId, ItemIndex};
use scale_info::TypeInfo;
use sp_core::H160;
use sp_runtime::DispatchError;

/// Maximum number of mint selections in one runtime API request.
pub const MAX_PREVIEW_QUERIES: u32 = 32;

/// One credit and collection whose mint selection is previewed.
#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
pub struct PreviewQuery {
	/// The credit used as the Random draw or contract entropy.
	pub credit: NftClaimCredit,
	/// The registered collection the claim would mint into.
	pub collection: CollectionId,
}

/// The registered mechanism that selected a previewed item.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
pub enum SelectionKind {
	/// The credit was reduced modulo the collection's next item index.
	Random,
	/// The registered minter contract selected the item.
	Contract(H160),
}

/// A failure encountered by the real claim selection path.
#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
pub enum PreviewFailure {
	/// The collection has no claim registration.
	CollectionNotRegistered,
	/// The registered collection no longer exists.
	UnknownCollection,
	/// The collection owner differs from the owner who registered claims.
	CollectionOwnerChanged,
	/// Random selection has no allocated item index to draw from.
	NoItems,
	/// The selected item definition has been deleted or does not exist.
	UnknownItem { item: ItemIndex },
	/// The registered contract rejected, trapped or returned an invalid result.
	ContractSelectionFailed { error: DispatchError },
}

/// The item a claim would mint or the selection failure it would return.
#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
pub enum PreviewOutcome {
	/// Selection succeeded and the named item definition exists.
	Mints { item: ItemIndex, via: SelectionKind },
	/// Selection failed before minting.
	Fails { reason: PreviewFailure },
}

/// A preview batch request that cannot be evaluated.
#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
pub enum BatchError {
	/// The batch exceeds the stable request ceiling and must be split by the caller.
	TooLarge { max: u32 },
}

sp_api::decl_runtime_apis! {
	/// Read-only previews of the item each NFT claim credit selects.
	pub trait NftClaimsApi {
		/// Executes the claim selector once per query and returns positionally aligned outcomes.
		/// Contract state changes occur only in the runtime API overlay and are discarded with it.
		/// Queries run sequentially on that one overlay, so two queries against the same stateful
		/// minter compound, exactly as claiming in that order would.
		fn preview_mints(
			queries: Vec<PreviewQuery>,
		) -> Result<Vec<PreviewOutcome>, BatchError>;
	}
}
