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

//! Shared types for the NFT claim credit trees.
//! The game pallet builds the commitments on the People chain and ships them over XCM.
//! The nft-claims pallet receives them in a batch.

use alloc::vec::Vec;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use frame_support::{
	pallet_prelude::{BoundedVec, Get},
	CloneNoBound, DebugNoBound, EqNoBound, PartialEqNoBound,
};
use scale_info::TypeInfo;
use sp_core::H256;

use crate::identity::AccountOrPerson;

/// An NFT claim credit earned by a player.
/// Hashes one successful report of one player on another, in one round of one game.
/// The claim chain mints an NFT from it.
pub type NftClaimCredit = [u8; 32];

/// The Merkle leaf committing to one awarded NFT claim credit.
///
/// Binds the claimant in separately from the credit, which is itself a hash and so does not say
/// who may mint. Nothing else is added: the credit already commits to the game index, the round
/// and both players.
///
/// Kept distinct from [`CreditProofNode`], which is the same 32 bytes on the wire, because a leaf
/// is hashed again to form the bottom layer while a node hash is rehashed as it is.
#[derive(
	Encode,
	Decode,
	DecodeWithMemTracking,
	MaxEncodedLen,
	TypeInfo,
	Debug,
	Clone,
	Copy,
	PartialEq,
	Eq,
)]
pub struct NftClaimCreditLeaf(pub [u8; 32]);

impl AsRef<[u8]> for NftClaimCreditLeaf {
	fn as_ref(&self) -> &[u8] {
		&self.0
	}
}

/// One node hash of a block's credit tree.
/// Either a sibling the claim chain rehashes a leaf against, or the root it arrives at.
#[derive(
	Encode,
	Decode,
	DecodeWithMemTracking,
	MaxEncodedLen,
	TypeInfo,
	Debug,
	Clone,
	Copy,
	PartialEq,
	Eq,
)]
pub struct CreditProofNode(pub [u8; 32]);

impl From<H256> for CreditProofNode {
	fn from(hash: H256) -> Self {
		Self(hash.0)
	}
}

impl From<CreditProofNode> for H256 {
	fn from(node: CreditProofNode) -> Self {
		H256(node.0)
	}
}

/// The Merkle leaf committing to `credit` being owned by `claimant`.
/// Both chains hash it here, so the game chain commits to what the claim chain recomputes.
pub fn credit_leaf<AccountId: Encode>(
	claimant: &AccountOrPerson<AccountId>,
	credit: &NftClaimCredit,
) -> NftClaimCreditLeaf {
	NftClaimCreditLeaf((claimant, credit).using_encoded(sp_io::hashing::blake2_256))
}

/// The position of a credit tree in the order the game pallet queued them for delivery.
/// Contiguous, so the receiver can tell that a tree never arrived.
/// Award blocks are not contiguous, because a block that awarded no credit has no tree.
pub type TreeSequence = u64;

/// The People-chain block a set of NFT claim credits was awarded in.
/// Both chains key a credit tree by it.
/// Fixed to `u32` so the XCM payload and the claim chain's storage stay free of a foreign chain's
/// block number type.
pub type AwardBlock = u32;

/// The Merkle commitment to all NFT claim credits awarded in one block.
/// Sent to Asset Hub, where a claimant mints an NFT by proving their leaf against [`Self::root`].
#[derive(
	Encode,
	Decode,
	DecodeWithMemTracking,
	MaxEncodedLen,
	TypeInfo,
	Debug,
	Clone,
	Copy,
	PartialEq,
	Eq,
)]
pub struct NftClaimCreditTree {
	/// The game whose credits the tree commits to.
	/// Only one game runs at a time, so every leaf belongs to this game.
	/// Carried so a game's trees can be grouped without mapping blocks to games.
	pub game_index: u32,
	/// The binary Merkle root over the block's leaves, in award order.
	pub root: CreditProofNode,
	/// The number of leaves in the tree.
	/// A proof cannot be checked without it: the count decides how an odd layer was rehashed.
	/// Always this committed count, never one the claimant supplies: that would let them pick
	/// which hash path is checked.
	pub leaf_count: u32,
	/// The block's wall-clock time in seconds since the UNIX epoch.
	/// Useful to display the age of the tree.
	/// May be used by chain data consumers, not used in the runtime.
	pub timestamp: u32,
}

/// One credit tree's delivery to the claim chain, with its block and sequence.
///
/// A delivery is not an update: a block's root is committed once and never changes, so
/// redelivering one the claim chain already holds leaves its state as it is.
#[derive(
	Encode,
	Decode,
	DecodeWithMemTracking,
	MaxEncodedLen,
	TypeInfo,
	Debug,
	Clone,
	Copy,
	PartialEq,
	Eq,
)]
pub struct CreditTreeDelivery {
	/// The tree's position in the delivery order.
	/// `None` for a tree resent by `replay_credit_trees`, which the game pallet no longer holds a
	/// sequence for.
	/// The receiver leaves its gap tracking untouched for an unsequenced tree.
	pub sequence: Option<TreeSequence>,
	/// The block whose credits the tree commits to.
	pub block: AwardBlock,
	/// The commitment itself.
	pub tree: NftClaimCreditTree,
}

/// A batch of credit trees sent from the game chain to the claim chain in one XCM message.
#[derive(
	CloneNoBound,
	PartialEqNoBound,
	EqNoBound,
	Encode,
	Decode,
	DecodeWithMemTracking,
	DebugNoBound,
	TypeInfo,
	MaxEncodedLen,
)]
#[scale_info(skip_type_params(MaxTrees))]
pub struct CreditTreeBatch<MaxTrees: Get<u32>> {
	/// Unix timestamp in seconds when the batch was assembled on the game chain.
	pub source_time: u64,
	/// The trees in the batch, in ascending block order.
	pub trees: BoundedVec<CreditTreeDelivery, MaxTrees>,
}

/// What the game needs from the pallet that owns the NFT claim credits.
///
/// The credits are bookkeeping the game triggers but does not own: a `Person` vote earns one, the
/// attendance backfill completes the set, and a game that ends or is cancelled gives up its slots.
/// The pallet holding them reads the game's own state, so the dependency runs that way and the
/// game reaches it through this trait.
///
/// The `award_*` methods return how many credits were really awarded, which is fewer than asked
/// for whenever a slot was already taken or the block had no room, and is what a caller debits
/// from the capacity it reserved.
pub trait AwardCredits<AccountId> {
	/// Award the credit `attester` earns `attestee` by reporting them a person in `round` of
	/// `game_index`, from `attester_position` in their group.
	fn award_report_credit(
		game_index: u32,
		round: u8,
		attester: &AccountOrPerson<AccountId>,
		attestee: &AccountOrPerson<AccountId>,
		attester_position: u32,
		award_time: u32,
	) -> u32;

	/// Award every credit a freshly attended `attendee` is owed for `game_index`, one per other
	/// member of their group in each round they played, skipping the ones already awarded.
	fn award_attendance_credits(
		game_index: u32,
		rounds: u8,
		max_group_size: u32,
		player_count: u32,
		attendee: &AccountOrPerson<AccountId>,
		award_time: u32,
	) -> u32;

	/// How many further credits the current block can award.
	///
	/// A caller that cannot split its awards across blocks checks this first; the value falls as
	/// the block awards.
	fn remaining_capacity() -> u32;

	/// Clear up to `limit` of `game_index`'s awarded-credit slots, resuming from `cursor`.
	/// Returns where to resume, or `None` once the game has none left.
	fn clear_game_credits(game_index: u32, limit: u32, cursor: Option<&[u8]>) -> Option<Vec<u8>>;

	/// Give up the awarded-credit slots `player` holds in `game_index`.
	fn forget_player_credits(game_index: u32, player: &AccountOrPerson<AccountId>);

	/// Mark every credit slot of `player` in `game_index` as awarded.
	///
	/// Setup a benchmark cannot do for itself: giving up those slots is work the game is charged
	/// for, and a slot set that was never filled would measure none of it.
	#[cfg(feature = "runtime-benchmarks")]
	fn benchmark_award_every_slot(game_index: u32, player: &AccountOrPerson<AccountId>);
}

/// For a runtime that plays games without minting anything from them.
impl<AccountId> AwardCredits<AccountId> for () {
	fn award_report_credit(
		_: u32,
		_: u8,
		_: &AccountOrPerson<AccountId>,
		_: &AccountOrPerson<AccountId>,
		_: u32,
		_: u32,
	) -> u32 {
		0
	}

	fn award_attendance_credits(
		_: u32,
		_: u8,
		_: u32,
		_: u32,
		_: &AccountOrPerson<AccountId>,
		_: u32,
	) -> u32 {
		0
	}

	fn remaining_capacity() -> u32 {
		u32::MAX
	}

	fn clear_game_credits(_: u32, _: u32, _: Option<&[u8]>) -> Option<Vec<u8>> {
		None
	}

	fn forget_player_credits(_: u32, _: &AccountOrPerson<AccountId>) {}

	#[cfg(feature = "runtime-benchmarks")]
	fn benchmark_award_every_slot(_: u32, _: &AccountOrPerson<AccountId>) {}
}
