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

//! Runtime API definition for Scarcity metadata layers.

use crate::{CollectionId, InstanceId, ItemIndex};
use alloc::vec::Vec;
use codec::{Decode, DecodeWithMemTracking, Encode};
use scale_info::TypeInfo;

/// Maximum number of metadata targets in one runtime API request.
pub const MAX_METADATA_QUERIES: u32 = 128;

/// A target whose stored metadata layers are requested.
#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
pub enum MetadataQuery {
	/// A live instance and all three metadata layers that apply to it.
	Instance(InstanceId),
	/// An existing item definition and its item and collection layers.
	Item { collection: CollectionId, item: ItemIndex },
	/// An existing collection and its collection layer.
	Collection(CollectionId),
}

/// The existing target a metadata query resolved to.
#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
pub enum MetadataTarget {
	/// A live instance together with the item definition it uses.
	Instance { instance: InstanceId, collection: CollectionId, item: ItemIndex },
	/// An existing item definition.
	Item { collection: CollectionId, item: ItemIndex },
	/// An existing collection.
	Collection(CollectionId),
}

/// Raw stored metadata split by scope for one query.
///
/// The pallet resolves effective values in instance override, item default then collection
/// default order. Clients can reproduce that resolution by merging these layers in reverse order.
#[derive(Clone, Default, PartialEq, Eq, Debug, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
pub struct MetadataLayers {
	/// The existing target and its resolved identifiers, or `None` when it does not exist.
	pub resolved: Option<MetadataTarget>,
	/// Collection defaults as raw key and value bytes.
	pub collection: Vec<(Vec<u8>, Vec<u8>)>,
	/// Item defaults as raw key and value bytes.
	pub item: Vec<(Vec<u8>, Vec<u8>)>,
	/// Instance overrides as raw key and value bytes.
	pub instance: Vec<(Vec<u8>, Vec<u8>)>,
}

/// A metadata batch request that cannot be evaluated.
#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
pub enum BatchError {
	/// The batch exceeds the stable request ceiling and must be split by the caller.
	TooLarge { max: u32 },
}

sp_api::decl_runtime_apis! {
	/// Read-only batched access to Scarcity's stored metadata layers.
	pub trait ScarcityApi {
		/// Returns one positionally aligned result per query without failing on missing targets.
		/// An oversized request fails explicitly and is never truncated.
		fn metadata_batch(
			queries: Vec<MetadataQuery>,
		) -> Result<Vec<MetadataLayers>, BatchError>;
	}
}
