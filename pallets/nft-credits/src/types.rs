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

//! The types the NFT claim credits are held and served in.
//!
//! The credit, its leaf and the tree committing to a block's leaves live in `indiv-support`,
//! because the claims chain hashes and stores the very same values. What is here is what only the
//! awarding side needs, and is re-exported from the crate root.

use alloc::vec::Vec;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use indiv_pallet_game::GameIdx;
use indiv_support::{
	credit_trees::{CreditProofNode, NftClaimCredit},
	identity::AccountOrPerson,
};
use scale_info::TypeInfo;

/// One credit of one claimant in one game, as a position in [`AwardedCredits`], derived from
/// the round and the [`indiv_pallet_game::AttesterPosition`] by `Pallet::credit_slot`.
pub type CreditSlot = u32;

/// The set of credits one claimant has been awarded in one game, held as one bit per
/// [`CreditSlot`].
///
/// [`Self::CAPACITY`] caps how many slots a game can use per claimant, which the pallet's
/// `integrity_test` holds the game's `MaxRounds * MaxGroupSize` to. A slot
/// beyond it has nowhere to be recorded, so the set reports it absent and refuses to insert
/// it, leaving [`Self::within_capacity`] as the check a caller makes once before relying on
/// either.
#[derive(
	Encode,
	Decode,
	DecodeWithMemTracking,
	MaxEncodedLen,
	TypeInfo,
	Debug,
	Clone,
	Copy,
	Default,
	PartialEq,
	Eq,
)]
pub struct AwardedCredits(u128);

impl AwardedCredits {
	/// The number of credit slots the set holds.
	pub const CAPACITY: u32 = u128::BITS;

	/// Every slot awarded, for benchmarks that need the worst case.
	#[cfg(feature = "runtime-benchmarks")]
	pub const FULL: Self = Self(u128::MAX);

	/// Whether `slot` is within [`Self::CAPACITY`], and so representable at all.
	pub const fn within_capacity(slot: CreditSlot) -> bool {
		slot < Self::CAPACITY
	}

	/// Whether `slot`'s credit is awarded. A slot the set cannot hold never is.
	pub fn contains(&self, slot: CreditSlot) -> bool {
		Self::bit(slot).is_some_and(|bit| self.0 & bit != 0)
	}

	/// Record `slot`'s credit as awarded. A slot the set cannot hold is not recorded.
	pub fn insert(&mut self, slot: CreditSlot) {
		if let Some(bit) = Self::bit(slot) {
			self.0 |= bit;
		}
	}

	/// How many of the claimant's credits this game has awarded.
	pub fn count(&self) -> u32 {
		self.0.count_ones()
	}

	/// The bit standing for `slot`, `None` beyond [`Self::CAPACITY`].
	const fn bit(slot: CreditSlot) -> Option<u128> {
		1u128.checked_shl(slot)
	}
}

/// One NFT claim credit as its block awarded it, which is the preimage of one
/// [`indiv_support::credit_trees::NftClaimCreditLeaf`].
///
/// Kept per award block in [`crate::NftClaimCreditAwards`] for as long as the block's awards are
/// retained, so a claim can be proven from state alone. Distinct from
/// [`crate::AwardedNftClaimCredits`], which only marks which of a game's credit slots a claimant
/// has had awarded.
#[derive(
	Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo, Debug, Clone, PartialEq, Eq,
)]
pub struct NftClaimCreditAward<AccountId> {
	/// Who the credit was awarded to, and who alone may mint against its leaf.
	pub claimant: AccountOrPerson<AccountId>,
	/// The credit awarded.
	pub credit: NftClaimCredit,
}

/// The fields a block's `NftClaimCreditTree` carries besides the root, recorded when the
/// block's first credit is awarded and read back when the root is computed.
///
/// They are kept alongside the leaves rather than derived when the root is computed: the game can
/// be over by then, so its index is no longer readable.
#[derive(
	Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo, Debug, Clone, PartialEq, Eq,
)]
pub struct NftClaimCreditRootInfo {
	/// The game the block's credits were awarded in.
	pub game_index: GameIdx,
	/// The block's wall-clock time in seconds since the UNIX epoch.
	pub timestamp: u32,
}

/// The inclusion proof of one NFT claim credit against the `NftClaimCreditTree` of the block it
/// was awarded in, as returned by [`crate::Pallet::nft_claim_credit_proofs`].
///
/// Carries only what the claims chain accepts from the claimant.
#[derive(Encode, Decode, DecodeWithMemTracking, TypeInfo, Debug, Clone, PartialEq, Eq)]
pub struct NftClaimCreditProof {
	/// The credit being claimed. The verifier hashes it with the claimant it authenticated to get
	/// the leaf, so somebody else's credit builds a different leaf and does not rehash to the
	/// stored root.
	pub credit: NftClaimCredit,
	/// The position of the credit's leaf in the block's leaves, in award order.
	pub leaf_index: u32,
	/// The sibling hashes that rehash the leaf up to the block's root, bottom layer first.
	pub proof: Vec<CreditProofNode>,
}

/// Why no [`NftClaimCreditProof`] could be built for a claim.
#[derive(Encode, Decode, DecodeWithMemTracking, TypeInfo, Debug, Clone, PartialEq, Eq)]
pub enum NftClaimCreditProofError {
	/// The block has no `NftClaimCreditTree`, so it awarded no credit.
	UnknownAwardBlock,
	/// The block's awards are no longer on chain, its root having dropped out of the retained
	/// window. The awards have to be supplied from the block's `NftClaimCreditAwarded` events
	/// instead.
	AwardsPruned,
	/// The given awards are not as many as the block's root was computed over.
	LeafCountMismatch {
		/// The number of leaves the root was computed over.
		expected: u32,
	},
	/// `leaf_index` is not a leaf of the block's tree.
	LeafIndexOutOfBounds,
	/// The given awards rehash to a different root than the one recorded for the block, so they
	/// are not the block's awards, or not in award order.
	RootMismatch,
}
