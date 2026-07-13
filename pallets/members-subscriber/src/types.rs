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

//! Types for the members-subscriber pallet.

use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use frame_support::{
	pallet_prelude::{BoundedBTreeMap, BoundedBTreeSet, Get},
	CloneNoBound, DebugNoBound, DefaultNoBound, EqNoBound, PartialEqNoBound,
};
pub use indiv_support::{
	members_notifier_subscriber::{MembersTypeConfig, RingRootOp, SequenceNumber},
	traits::{Alias, Context, Identifier, RevisionIndex, RingIndex},
};
use scale_info::TypeInfo;
use verifiable::DecodeUnchecked;

use xcm::v5::Location;

use super::Config;

/// Bridges the subscriber pallet's `Config` to the shared `MembersTypeConfig` trait,
/// allowing shared types (`RingRootUpdate`, `RingRootUpdatesBatch`, `MembersOf`)
/// to be parameterized over subscriber's configuration.
pub struct SubscriberConfig<T>(core::marker::PhantomData<T>);

impl<T: Config> MembersTypeConfig for SubscriberConfig<T> {
	type Crypto = T::Crypto;
	type MaxUpdatesPerBatch = T::MaxUpdatesPerBatch;
	type MaxCollections = T::MaxCollections;
}

/// Ring root members type from the crypto implementation.
pub type MembersOf<T> = indiv_support::members_notifier_subscriber::MembersOf<SubscriberConfig<T>>;

/// Represents a single ring root update sent between notifier and subscriber.
pub type RingRootUpdate<T> =
	indiv_support::members_notifier_subscriber::RingRootUpdate<SubscriberConfig<T>>;

/// Batch of ring root updates for a single collection, sent between notifier and subscriber.
pub type RingRootUpdatesBatch<T> =
	indiv_support::members_notifier_subscriber::RingRootUpdatesBatch<SubscriberConfig<T>>;

/// Record with ring commitment received from the notifier.
/// Stored locally on subscriber chains and contains all information needed
/// to verify personhood proofs against the ring.
///
/// The `Decode` impl is hand-written rather than derived: `root` routes
/// through [`DecodeUnchecked::decode_unchecked`], skipping arkworks
/// curve-point validation on storage reads. The value was validated at
/// ingress (XCM payload from the notifier), so revalidating every read
/// would be wasted work.
///
/// **Warning**: This type contains a `root` which is trusted and decoded without check, it must
/// have been validated
#[derive(
	Encode,
	DecodeWithMemTracking,
	CloneNoBound,
	PartialEqNoBound,
	EqNoBound,
	DebugNoBound,
	TypeInfo,
	MaxEncodedLen,
)]
#[scale_info(skip_type_params(T))]
pub struct RingCommitmentRecord<T: Config> {
	/// The ring root commitment value used for membership verification.
	pub root: MembersOf<T>,
	/// Revision number of this ring root (incremented on each update).
	pub revision: RevisionIndex,
	/// Unix timestamp in seconds when the batch was created on the source chain.
	pub source_time: u64,
	/// Sequence number from the source batch.
	pub source_sequence: SequenceNumber,
}

impl<T: Config> Decode for RingCommitmentRecord<T> {
	fn decode<I: codec::Input>(input: &mut I) -> Result<Self, codec::Error> {
		Ok(Self {
			root: <MembersOf<T> as DecodeUnchecked>::decode_unchecked(input)?,
			revision: Decode::decode(input)?,
			source_time: Decode::decode(input)?,
			source_sequence: Decode::decode(input)?,
		})
	}
}

/// State of a ring collection.
#[derive(
	Encode,
	Decode,
	DecodeWithMemTracking,
	CloneNoBound,
	PartialEqNoBound,
	EqNoBound,
	DebugNoBound,
	TypeInfo,
	MaxEncodedLen,
	DefaultNoBound,
)]
#[scale_info(skip_type_params(MaxMissing, MaxDeleted))]
pub struct RingCollectionState<MaxMissing: Get<u32>, MaxDeleted: Get<u32>> {
	/// Number of unique ring roots stored for this collection.
	pub ring_count: u32,
	/// Upper bound of the ring index space (max allocated index + 1).
	/// Updated from each batch's `next_ring_index`. Used as the scan range for missing detection.
	pub next_ring_index: u32,
	/// Missing ring indices mapped to their replay request attempt count.
	/// Count starts at 0 when first detected, increments on each replay request.
	pub missing_indices: BoundedBTreeMap<RingIndex, u32, MaxMissing>,
	/// Ring indices known to have been deleted by the notifier.
	/// Tracked to avoid falsely marking deleted rings as missing.
	pub deleted_indices: BoundedBTreeSet<RingIndex, MaxDeleted>,
}

/// Subscription status tracking.
#[derive(
	Encode,
	Decode,
	DecodeWithMemTracking,
	Clone,
	PartialEq,
	Eq,
	Debug,
	TypeInfo,
	MaxEncodedLen,
	Default,
)]
pub enum SubscriptionStatus {
	#[default]
	Inactive,
	Active {
		/// Sequence number of the initialization batch.
		initialized_at_sequence: SequenceNumber,
	},
	Terminated,
}

/// State for tracking updates processing timestamps and sequence numbers.
#[derive(
	Encode,
	Decode,
	DecodeWithMemTracking,
	Clone,
	PartialEq,
	Eq,
	Debug,
	TypeInfo,
	MaxEncodedLen,
	Default,
)]
pub struct UpdatesProcessingState {
	/// Sequence number of the last successfully processed ring root updates batch.
	pub last_processed_sequence: SequenceNumber,
	/// Unix timestamp (in seconds) when the last batch was received.
	pub last_batch_received_time: u64,
	/// Unix timestamp (in seconds) when the last replay request was sent.
	pub last_replay_request_time: u64,
}

/// Endpoint for communicating with the notifier.
/// Bundles the XCM location and pallet index into a single configuration type.
#[derive(
	Clone, PartialEq, Eq, Debug, Encode, Decode, TypeInfo, MaxEncodedLen, DecodeWithMemTracking,
)]
pub struct NotifierEndpoint {
	/// XCM location of the notifier chain from this chain's perspective.
	pub location: Location,
	/// Pallet index of members-notifier on the notifier chain.
	/// Used for XCM Transact call encoding.
	pub pallet_index: u8,
}
