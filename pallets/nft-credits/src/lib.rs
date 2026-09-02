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

//! # NFT Credits Pallet
//!
//! The credits a game awards, the Merkle trees committing to them and their delivery to the chain
//! that mints the NFTs.
//!
//! A credit is earned by playing, so the game pallet is what triggers an award: `report` awards one
//! per `Person` vote and the attendance backfill completes the set once a player's attendance is
//! final. Everything that happens to a credit afterwards is self-contained and lives here: the
//! per-block tree built over the awards, the queue of trees owed to the claims chain, the XCM that
//! carries them, and the proofs a claimant needs.
//!
//! ## Why a pallet of its own
//!
//! This is the game's own bookkeeping, but none of it is the game: it has its own calls, its own
//! events and its own storage, and mixing them into the game pallet's put three unrelated
//! dispatchables in the same call enum. It sits *above* the game instead ([`Config`] requires
//! `indiv_pallet_game::Config`), so it reads the game's groups and rounds directly, while the game
//! reaches back through [`indiv_support::credit_trees::AwardCredits`] and knows nothing of credits
//! beyond that trait.
//!
//! ## Awarding
//!
//! [`AwardedNftClaimCredits`] marks which of a game's credit slots a claimant already holds, so a
//! credit is awarded once however many times both award paths reach it. Each award is appended to
//! the block's [`NftClaimCreditAwards`] and emitted as `NftClaimCreditAwarded`, in leaf order.
//!
//! ## Committing
//!
//! `on_initialize` builds one binary Merkle tree over the previous block's awards and records its
//! root in [`NftClaimCreditRoots`] under the block the credits were awarded in. Blocks that awarded
//! no credit are skipped. Each award contributes exactly one leaf, `blake2_256` over the
//! SCALE-encoded `(claimant, credit)`.
//!
//! One root per block, rather than one per game, lets a claimant mint as soon as the root
//! committing to their credit reaches the minting chain. Each root stands alone and never changes
//! afterwards, so no inclusion proof goes stale.
//!
//! The tree itself is never stored, only its root, and its leaves are recoverable two ways:
//!
//! - From [`NftClaimCreditAwards`], for as long as the award block is one of the
//!   [`Config::MaxRetainedAwardBlocks`] most recent. This is the intended path and needs nothing
//!   but chain state.
//! - From the block's `NftClaimCreditAwarded` events, one per awarded credit, carrying the
//!   claimant, the credit and the leaf index. This is the fallback once a block's awards have been
//!   pruned.
//!
//! What the chain keeps of an award beyond that window is the leaf inside its block's root, which
//! is kept for good, and the slot in [`AwardedNftClaimCredits`] that stops the credit being awarded
//! a second time. A claimant, or an indexer, that held on to the awards can still mint.
//!
//! ## Delivering
//!
//! Every recorded root is queued in [`CreditTreeDeliveryQueue`] under a contiguous sequence number,
//! and the offchain worker ships a message's worth per block with [`Pallet::send_credit_trees`]. A
//! tree that never arrives is repaired by [`Pallet::replay_credit_trees`], which anyone may call
//! and which carries no sequence number. One replay allowed per [`Config::ReplayCooldownSeconds`],
//! so as not to congest the channel.
//!
//! ## Claiming
//!
//! Claiming happens on the claims chain, which never sees the credits themselves, only one root per
//! block. A claimant proves their entitlement by presenting their credit with an inclusion proof:
//! the sibling hashes that rehash the credit's leaf up to the root held for the block the credit
//! was awarded in. The claims chain builds the leaf itself, from the credit presented and the
//! claimant its origin authenticated. Presenting somebody else's credit builds a different leaf,
//! which does not rehash to the stored root.
//!
//! A claimant does not have to rebuild the tree themselves. The runtime API in [`runtime_api`]
//! serves the proof material:
//!
//! - `nft_claim_credit_roots` resolves [`NftClaimCreditBlocks`], which maps a claimant to the
//!   blocks they were awarded a credit in, against [`NftClaimCreditRoots`], so a claimant finds
//!   their roots by one lookup instead of a scan.
//! - `nft_claim_credit_proofs` returns, for one award block and one claimant, the inclusion proof
//!   of each credit the claimant holds there: the credit, its leaf index and the sibling hashes,
//!   which is what the claims chain verifies. `nft_claim_credit_proof_from_awards` does the same
//!   for a pruned block, from awards the caller supplies.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
#[cfg(test)]
mod mock;
pub mod runtime_api;
#[cfg(test)]
mod tests;
mod types;
pub mod weights;

pub use pallet::*;
pub use types::*;
pub use weights::WeightInfo;

use alloc::{vec, vec::Vec};
use codec::Compact;
use core::marker::PhantomData;
use cumulus_primitives_core::{GetChannelInfo, ParaId};
use frame_support::{
	defensive,
	pallet_prelude::*,
	traits::{Defensive, DefensiveOption, UnixTime},
};
use frame_system::{
	offchain::{CreateAuthorizedTransaction, SubmitTransaction},
	pallet_prelude::*,
};
use indiv_pallet_game::{
	AttesterPosition, GameIdx, GroupsSetting, IndexToPlayer, PlayerToIndex, RoundIndex,
};
use indiv_support::{
	credit_trees::{
		AwardBlock, AwardCredits, CreditProofNode, CreditTreeDelivery, NftClaimCredit,
		NftClaimCreditLeaf, NftClaimCreditTree, TreeSequence,
	},
	identity::AccountOrPerson,
	tx_priority,
	weight_budget::OcwWeightBudget,
};
use sp_runtime::{traits::BlakeTwo256, SaturatedConversion, Saturating};
use xcm::{
	latest::{
		Instruction::{Transact, UnpaidExecution},
		Junction::Parachain,
		Location, OriginKind, SendXcm, WeightLimit, Xcm,
	},
	prelude::send_xcm,
	VersionedXcm,
};

/// Retry window, in blocks, for the offchain worker's [`Pallet::send_credit_trees`].
/// Retries within one window are byte-identical, so the transaction pool deduplicates them.
/// A new window changes the discriminator and thus the transaction hash, which escapes both that
/// deduplication and the pool rotator's inclusion ban.
const CREDIT_TREE_RETRY_WINDOW: u32 = 8;

/// Finite longevity for [`Pallet::send_credit_trees`] so that a stranded retry self-evicts from the
/// pool rather than lingering until it is mined against state it no longer matches.
const CREDIT_TREE_TX_LONGEVITY: u64 = 64;

/// Period, in blocks, at which a failing [`Pallet::send_credit_trees`] submission is warned about
/// rather than logged at `debug`. A stalled delivery is otherwise only visible as a queue that
/// stops draining.
const CREDIT_TREE_STALL_WARN_PERIOD: u32 = 32;

/// Bytes held back from the claims channel's per-message room for what the router adds to a credit
/// tree message after the pallet has handed it over.
///
/// A router appends a `SetTopic` (33 bytes), and the XCMP queue counts the page format byte and,
/// on a channel that takes opaque fragments, the fragment's own length prefix against the same
/// per-message room. None of that is visible to the pallet, so the size it computes is short by up
/// to about 40 bytes and a channel filled to the byte would reject the message.
const CREDIT_TREE_ROUTER_HEADROOM: usize = 64;

const LOG_TARGET: &str = "runtime::indiv-pallet-nft-credits";

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	pub struct Pallet<T>(PhantomData<T>);

	/// The credits are the game's own bookkeeping, so this pallet is configured on top of it and
	/// reads its groups, rounds and player indices directly.
	#[pallet::config]
	pub trait Config:
		frame_system::Config + indiv_pallet_game::Config + CreateAuthorizedTransaction<Call<Self>>
	{
		/// Weight information for the extrinsics and hooks of this pallet.
		type WeightInfo: WeightInfo;

		/// What the benchmarks cannot set up themselves, because only the runtime knows how its
		/// XCM channels are made.
		#[cfg(feature = "runtime-benchmarks")]
		type BenchmarkHelper: crate::benchmarking::BenchmarkHelper;

		/// The maximum number of NFT claim credits that can be awarded in a single block, and
		/// hence the maximum number of leaves in one [`NftClaimCreditTree`].
		///
		/// One block's awards are a single [`NftClaimCreditAwards`] entry, whose proof size is
		/// charged at `max_size` on every award and on every root computation, so this bound is
		/// paid for in every `report` whether the awards are there or not.
		///
		/// An unrecorded credit is committed to no root and stays unmintable, and the pallet can
		/// only report that defensively, so the bound must cover what a whole block of `report`s
		/// awards. The `integrity_test` asserts that floor from the runtime's own block limits and
		/// weights, failing the `runtime_integrity_tests` that its `construct_runtime!` generates.
		/// The backfill in `player_process_step1` needs no floor: it defers a player that no
		/// longer fits.
		///
		/// Slack above the floor buys margin against a weight regeneration lifting it, and costs
		/// reports per block, since it is charged to every one of them. That charge also lowers the
		/// floor, so the margin grows faster than the slack. The `integrity_test` warns rather than
		/// fails above twice the floor.
		///
		/// Raising the bound in a runtime upgrade is safe. Lowering it strands every retained
		/// block's awards, since [`NftClaimCreditAwards`] no longer decodes against the smaller
		/// bound, so clear the map first.
		#[pallet::constant]
		type MaxCreditsPerBlock: Get<u32>;

		/// XCM sender used to deliver the credit trees to [`Config::NftClaimsParaId`].
		type XcmRouter: SendXcm;

		/// The parachain the credit trees are delivered to, which mints the NFTs claimed against
		/// them. There is exactly one, so the runtime fixes it rather than governance registering
		/// it.
		#[pallet::constant]
		type NftClaimsParaId: Get<ParaId>;

		/// Pallet index of indiv-pallet-nft-claims on [`Config::NftClaimsParaId`], used to
		/// encode the `Transact` call the trees are delivered in.
		#[pallet::constant]
		type NftClaimsPalletIndex: Get<u8>;

		/// Channel info provider, used to size a message to what the HRMP channel to
		/// [`Config::NftClaimsParaId`] can carry.
		type ChannelInfo: GetChannelInfo;

		/// The maximum number of credit trees that can wait for delivery in
		/// `CreditTreeDeliveryQueue`.
		///
		/// One block builds at most one tree and the offchain worker drains the queue every block,
		/// so this only has to cover an outage. A tree that does not fit is never queued and needs
		/// [`Pallet::replay_credit_trees`], so size it well past
		/// [`Config::MaxCreditTreesPerMessage`].
		#[pallet::constant]
		type MaxQueuedCreditTrees: Get<u32>;

		/// The maximum number of credit trees carried by one XCM message.
		///
		/// The nft-claims pallet's own bound must be at least this large, otherwise the batches
		/// sent to it fail to decode and the trees in them never arrive.
		#[pallet::constant]
		type MaxCreditTreesPerMessage: Get<u32>;

		/// Cooldown, in seconds, between credit tree replays.
		///
		/// [`Pallet::replay_credit_trees`] is permissionless, so this is what bounds the XCMP
		/// traffic it can cause.
		#[pallet::constant]
		type ReplayCooldownSeconds: Get<u64>;

		/// Per-tree weight surcharge for executing `receive_credit_trees` on
		/// [`Config::NftClaimsParaId`], charged to the caller of
		/// [`Pallet::replay_credit_trees`].
		///
		/// This prices the remote work a replay causes; [`Config::ReplayCooldownSeconds`] is what
		/// bounds how often one can happen. Set it to at least the per-tree cost of
		/// `receive_credit_trees` in the claims chain's own generated weights, proof size
		/// included.
		#[pallet::constant]
		type NftClaimsRemoteWeight: Get<Weight>;

		/// The number of most recent award blocks whose [`NftClaimCreditAwards`] stay on chain.
		///
		/// This is the window in which a claim can be proven from state alone, through
		/// [`Pallet::nft_claim_credit_proofs`]. Once a block drops out of it, its awards are
		/// removed and a proof has to be rebuilt from the block's `NftClaimCreditAwarded` events
		/// and passed to [`Pallet::nft_claim_credit_proof_from_awards`]. The root itself is kept
		/// for good, so dropping out delays no mint that a claimant, or an indexer, kept the
		/// awards of.
		///
		/// It counts award blocks, not blocks, because only blocks that awarded a credit have an
		/// entry. Sized against how long a claimant may take to mint, and paid for in state: the
		/// map holds at most this many entries of `MaxCreditsPerBlock` awards each.
		///
		/// Raising the bound in a runtime upgrade is safe. Lowering it orphans the awards of the
		/// blocks beyond the new bound, since `NftClaimCreditAwardBlocks` no longer decodes and
		/// the ring is what names the entries to remove, so clear the map first.
		#[pallet::constant]
		type MaxRetainedAwardBlocks: Get<u32>;

		/// The maximum number of award blocks [`NftClaimCreditBlocks`] keeps per claimant.
		///
		/// The index is a lookup aid, not the record of what a claimant is owed, so a full list
		/// drops its oldest block rather than rejecting an award. Size it past the blocks a
		/// claimant can earn credits in over the games whose trees are still worth minting
		/// against, and account for the proof size: a read is charged at the list's maximum
		/// encoded length, one block number per entry, once per distinct claimant an extrinsic
		/// awards to.
		#[pallet::constant]
		type MaxCreditBlocksPerClaimant: Get<u32>;
	}

	/// A batch of credit trees as it is sent to the NFT claims chain.
	pub type CreditTreeBatch<T> =
		indiv_support::credit_trees::CreditTreeBatch<<T as Config>::MaxCreditTreesPerMessage>;

	/// The calls of indiv-pallet-nft-claims that this pallet dispatches over XCM.
	///
	/// The variant's index and its field order must mirror the dispatchable on the claims chain.
	#[derive(Encode)]
	pub(crate) enum NftClaimsCall<T: Config> {
		#[codec(index = 0)]
		ReceiveCreditTrees { batch: CreditTreeBatch<T> },
	}

	/// Which credits a game has already awarded a claimant, keyed by game index and claimant.
	/// The value marks the slots of [`Pallet::credit_slot`] that are taken.
	///
	/// Bookkeeping for [`Pallet::award_nft_claim_credit`], its only reader, and what makes an
	/// award idempotent: `report` awards a `Person` vote's credit on the spot and
	/// `award_attendance_credits` later walks that same credit, which unguarded would give the
	/// claimant a second leaf, in a second block's tree, and two mints from one credit. Off
	/// chain it is not needed, Asset Hub minting against a root it is sent and a wallet reading
	/// [`NftClaimCreditBlocks`] and the blocks' `NftClaimCreditAwarded` events. One word per
	/// claimant rather than an entry per credit, because that entry count is what the backfill's
	/// proof size is made of.
	///
	/// Only the current game is ever in here, entries being drained before
	/// `player_process_step2` kills the game and `new_game` refusing to start one while a game
	/// exists. The game key still earns its place: a slot means nothing outside the game whose
	/// groups it indexes, so were a drain left half done, an unkeyed entry would read as the
	/// next game's awarded slots and silently swallow those credits.
	#[pallet::storage]
	pub type AwardedNftClaimCredits<T: Config> = StorageDoubleMap<
		_,
		Twox64Concat,
		GameIdx,
		Blake2_128Concat,
		AccountOrPerson<T::AccountId>,
		AwardedCredits,
		ValueQuery,
	>;

	/// The blocks a claimant was awarded at least one NFT claim credit in, in ascending order
	/// and without repeats.
	///
	/// Each entry keys an [`NftClaimCreditRoots`] entry whose tree holds a leaf of the claimant's,
	/// so this answers which roots the claimant has something to mint against without scanning
	/// every block. The proof itself still comes from that block's `NftClaimCreditAwarded` events;
	/// the index states which blocks to fetch.
	///
	/// Blocks are appended as credits are awarded and never removed once minted. The list is
	/// therefore a ring bounded by [`Config::MaxCreditBlocksPerClaimant`].
	#[pallet::storage]
	pub type NftClaimCreditBlocks<T: Config> = StorageMap<
		_,
		Blake2_128Concat,
		AccountOrPerson<T::AccountId>,
		BoundedVec<BlockNumberFor<T>, T::MaxCreditBlocksPerClaimant>,
		ValueQuery,
	>;

	/// The NFT claim credits each retained block awarded, in award order, which are the preimages
	/// of that block's Merkle leaves.
	///
	/// The current block's entry doubles as the buffer the next block's `on_initialize` computes
	/// the root over: `Pallet::award_nft_claim_credit` appends to it, and once the root is
	/// recorded the entry stays as it is, so a claim can be proven from state alone. The oldest
	/// entry is removed when a new root pushes it out of the
	/// [`Config::MaxRetainedAwardBlocks`] window, which is what bounds the map.
	///
	/// The awards are kept rather than the leaves they hash to, because a mint needs the credit
	/// itself: Asset Hub recomputes the leaf from the claimant and the credit the claimant
	/// presents, so a leaf alone would still leave the credit to be recovered from events.
	#[pallet::storage]
	pub type NftClaimCreditAwards<T: Config> = StorageMap<
		_,
		Twox64Concat,
		BlockNumberFor<T>,
		BoundedVec<NftClaimCreditAward<T::AccountId>, T::MaxCreditsPerBlock>,
		ValueQuery,
	>;

	/// The award blocks whose [`NftClaimCreditAwards`] are still on chain, in ascending order.
	///
	/// A ring bounded by [`Config::MaxRetainedAwardBlocks`]: recording a root appends its block
	/// and, when that fills the ring, removes the awards of the block that drops off the front.
	/// Keeping the list rather than pruning by block arithmetic means no block ever pays for the
	/// removal of an entry that was never there, award blocks being sparse.
	#[pallet::storage]
	pub type NftClaimCreditAwardBlocks<T: Config> =
		StorageValue<_, BoundedVec<BlockNumberFor<T>, T::MaxRetainedAwardBlocks>, ValueQuery>;

	/// The fields the [`NftClaimCreditRoots`] entry of the current block will carry besides the
	/// root. Written when the block's first credit is awarded and cleared once its root is
	/// recorded.
	///
	/// `None` exactly when the current block has awarded no credit.
	/// [`Pallet::build_credit_tree`] relies on that to decide whether a block has a root to
	/// record without reading the block's awards, which are charged at `max_size`.
	#[pallet::storage]
	pub type PendingNftClaimCreditRootInfo<T: Config> = StorageValue<_, NftClaimCreditRootInfo>;

	/// The Merkle commitment to each block's awarded NFT claim credits, keyed by the block the
	/// credits were awarded in. Blocks that awarded no credit have no entry.
	#[pallet::storage]
	pub type NftClaimCreditRoots<T: Config> =
		StorageMap<_, Twox64Concat, BlockNumberFor<T>, NftClaimCreditTree>;

	/// The award blocks whose credit trees have not been delivered to the NFT claims chain yet,
	/// in ascending block order, each with the sequence number it is delivered under.
	///
	/// Appended by [`Pallet::build_credit_tree`] and drained from the front by
	/// [`Pallet::send_credit_trees`] once the XCM carrying the trees has been accepted for
	/// delivery. The trees themselves stay in [`NftClaimCreditRoots`]; this only records what
	/// still owes a delivery.
	#[pallet::storage]
	pub type CreditTreeDeliveryQueue<T: Config> = StorageValue<
		_,
		BoundedVec<(TreeSequence, BlockNumberFor<T>), T::MaxQueuedCreditTrees>,
		ValueQuery,
	>;

	/// The time of the last credit tree replay, in seconds.
	///
	/// [`Pallet::replay_credit_trees`] refuses to run again within
	/// [`Config::ReplayCooldownSeconds`] of it.
	#[pallet::storage]
	pub type LastReplayTime<T: Config> = StorageValue<_, u64, OptionQuery>;

	/// The sequence number the next queued credit tree is delivered under.
	///
	/// Only a tree that made it into [`CreditTreeDeliveryQueue`] consumes one, so the sequence the
	/// claims chain sees stays contiguous even when the queue overflows. A gap there therefore
	/// means a message was lost, never that one was not sent.
	#[pallet::storage]
	pub type NextCreditTreeSequence<T: Config> = StorageValue<_, TreeSequence, ValueQuery>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// An NFT claim credit was awarded to `claimant` and recorded as leaf `leaf_index` of the
		/// current block's tree.
		///
		/// One event per awarded credit, in leaf order, is what lets an inclusion proof still be
		/// built once the block's awards have been pruned: the leaf is
		/// `blake2_256(claimant ++ credit)` and the block's leaf set is the `leaf_index`-ordered
		/// sequence of these events, so no block replay is needed.
		NftClaimCreditAwarded {
			claimant: AccountOrPerson<T::AccountId>,
			credit: NftClaimCredit,
			leaf_index: u32,
		},
		/// The credits awarded in block `block` can be minted from now on: `credit_root`'s root
		/// is what an inclusion proof for any of them verifies against, and never changes.
		NftClaimCreditRootRecorded { block: BlockNumberFor<T>, credit_root: NftClaimCreditTree },
		/// Credit trees were handed to the XCM router for delivery to the NFT claims chain.
		CreditTreesSent {
			/// The award block of every tree the message carries, in the order they go out.
			/// Empty means every tree of this message had lost its root, so nothing was sent.
			///
			/// The sequence each block travels under is not spelled out: the message takes a
			/// contiguous run of sequences off the queue, starting at the call's `first_sequence`,
			/// minus the ones this block's [`Event::CreditTreeDeliverySkipped`] names.
			trees: BoundedVec<AwardBlock, T::MaxCreditTreesPerMessage>,
		},
		/// A queued credit tree was not sent because its root is no longer recorded.
		///
		/// Its sequence is spent without a tree ever arriving, so the claims chain reports a gap.
		/// No [`Pallet::replay_credit_trees`] can fill it: the root proofs verify against is gone.
		CreditTreeDeliverySkipped { sequence: TreeSequence, block: BlockNumberFor<T> },
		/// Delivering credit trees to the NFT claims chain failed. The trees stay queued and the
		/// next offchain worker cycle retries them.
		CreditTreeSendFailed,
		/// Credit trees were resent to the NFT claims chain out of band.
		CreditTreesReplayed { count: u32 },
		/// A freshly built credit tree could not be queued for delivery because
		/// `CreditTreeDeliveryQueue` is full, which means delivery has been failing for
		/// `MaxQueuedCreditTrees` trees. Its credits stay unmintable until a
		/// [`Pallet::replay_credit_trees`] names `block`.
		CreditTreeDeliveryDropped { block: BlockNumberFor<T> },
	}

	#[pallet::error]
	pub enum Error<T> {
		/// A credit tree replay was requested for an empty list of blocks.
		NoBlocksToReplay,
		/// The blocks to replay are not in strictly ascending order.
		UnsortedReplayBlocks,
		/// None of the blocks to replay has a credit tree.
		NoCreditTreeForBlock,
		/// A credit tree replay ran within `ReplayCooldownSeconds` of the last one. The window is
		/// shared by every caller.
		ReplayCooldownActive,
		/// The replay does not fit what the HRMP channel to the NFT claims chain can carry in
		/// one message.
		ExceedsClaimsChannelCapacity,
		/// Sending the credit trees to the NFT claims chain over XCM failed.
		CreditTreeXcmFailed,
		/// The round is not below `MaxRounds`, or the attester slot is not below `MaxGroupSize`,
		/// so the two name a credit slot no game can use.
		#[cfg(feature = "testnet")]
		CreditSlotOutOfBounds,
		/// No credit was awarded: the claimant already holds the slot's credit for that game, or
		/// the block has no room for another award.
		#[cfg(feature = "testnet")]
		CreditNotAwarded,
	}

	pub enum AuthorizeInvalidity {
		/// Transaction source is not local or in block.
		TransactionNotLocal = 200,
		/// No credit tree is waiting to be delivered to the NFT claims chain.
		NoQueuedCreditTrees = 201,
	}

	impl From<AuthorizeInvalidity> for TransactionValidityError {
		fn from(e: AuthorizeInvalidity) -> Self {
			InvalidTransaction::Custom(e as u8).into()
		}
	}

	/// A reason for this pallet placing a hold on funds.
	#[pallet::composite_enum]
	pub enum HoldReason {
		/// Native balance held as the signup deposit for account-based players.
		PlayDeposit,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		#[cfg(feature = "std")]
		fn integrity_test() {
			Self::integrity_test_credits();
		}

		/// Builds the tree over the previous block's awards, which are complete by now.
		fn on_initialize(n: BlockNumberFor<T>) -> Weight {
			Self::build_credit_tree(n)
		}

		fn offchain_worker(block_number: BlockNumberFor<T>) {
			Self::submit_credit_tree_delivery(block_number);
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Delivers the queued credit trees that fit one XCM message to the NFT claims chain.
		///
		/// Authorized call submitted by this pallet's offchain worker: it is accepted from a
		/// local or in-block source only, so it cannot be submitted externally.
		///
		/// `first_sequence` must be the sequence at the front of `CreditTreeDeliveryQueue`, which
		/// makes a retry that raced a successful delivery stale rather than a second send.
		#[pallet::authorize(|source, first_sequence, _discriminator| {
			Self::authorize_send_credit_trees(source, first_sequence)
		})]
		#[pallet::call_index(18)]
		#[pallet::weight(<T as Config>::WeightInfo::send_credit_trees(
			T::MaxCreditTreesPerMessage::get()
		))]
		#[pallet::weight_of_authorize(<T as Config>::WeightInfo::authorize_send_credit_trees())]
		pub fn send_credit_trees(
			origin: OriginFor<T>,
			_first_sequence: TreeSequence,
			// Per-window discriminator (the submitting retry window) so that a stalled
			// offchain-worker retry eventually produces a fresh transaction hash.
			_discriminator: BlockNumberFor<T>,
		) -> DispatchResultWithPostInfo {
			ensure_authorized(origin)?;

			Self::do_send_credit_trees()
		}

		/// Resends the credit trees of `blocks` to the NFT claims chain.
		///
		/// Permissionless: a credit tree is a public commitment the claims chain is meant to hold,
		/// and the trees are read from [`NftClaimCreditRoots`] rather than supplied by the caller.
		/// Blocks without a tree are skipped. A resent tree carries no sequence number, so this
		/// cannot disturb the claims chain's tracking of the live stream, and one the claims chain
		/// already holds is ignored there rather than overwriting anything.
		///
		/// One replay runs per [`Config::ReplayCooldownSeconds`], counted from the last one by
		/// [`LastReplayTime`].
		///
		/// The caller pays for the remote work, [`Config::NftClaimsRemoteWeight`] per tree on top
		/// of this call's own weight.
		///
		/// ## Parameters
		/// - `blocks`: The award blocks to resend, in strictly ascending order.
		#[pallet::call_index(19)]
		#[pallet::weight(
			<T as Config>::WeightInfo::replay_credit_trees(blocks.len() as u32)
				.saturating_add(
					T::NftClaimsRemoteWeight::get().saturating_mul(blocks.len() as u64)
				)
		)]
		pub fn replay_credit_trees(
			origin: OriginFor<T>,
			blocks: BoundedVec<BlockNumberFor<T>, T::MaxCreditTreesPerMessage>,
		) -> DispatchResult {
			ensure_signed(origin)?;

			Self::do_replay_credit_trees(blocks)
		}

		/// Award an NFT claim credit to `claimant` outside of a game.
		///
		/// This action can only be performed by the root origin and is only meant for testing.
		/// It exists because a credit is otherwise only earned by a `Person` vote in a played
		/// game, which makes the claim chain's minting path hard to exercise on its own. The
		/// credit it awards is a normal one: it is recorded in this block's
		/// [`NftClaimCreditAwards`], committed to the block's tree and claimed with the same
		/// proof as any other.
		///
		/// The credit is the one `attester` would earn `claimant` by reporting them a person in
		/// `round` of `game_index`, so it does not need any of those to exist. A slot is used
		/// once per claimant and game: repeating a call with the same `game_index`, `round` and
		/// `attester_position` for the same `claimant` fails rather than awarding a second credit.
		/// Vary any of them to award more than one.
		///
		/// Parameters:
		/// - `claimant`: who the credit is awarded to, and the only identity that can mint it.
		/// - `attester`: the reporter the credit is attributed to, which along with `game_index`
		///   and `round` is what makes the credit distinct from another claimant's.
		/// - `game_index`: the game the credit is attributed to. All the credits a block awards
		///   must name the same game, since the block's tree is labelled with one.
		/// - `round`: the round of that game, below `MaxRounds`.
		/// - `attester_position`: the attester's place in the group, below `MaxGroupSize`. Together
		///   with `round` it picks the claimant's credit slot for the game.
		#[pallet::call_index(103)]
		#[pallet::weight(Weight::zero())]
		#[cfg(feature = "testnet")]
		pub fn testnet_grant_nft_claim_credit(
			origin: OriginFor<T>,
			claimant: AccountOrPerson<T::AccountId>,
			attester: AccountOrPerson<T::AccountId>,
			game_index: GameIdx,
			round: RoundIndex,
			attester_position: AttesterPosition,
		) -> DispatchResult {
			// The `testnet` feature is the only gate, unlike the calls above, which also read
			// `Config::TESTNET`. That constant switches game logic (`acceptable_player_count`)
			// as well, so it cannot be turned on in the mock, and a call behind it is untestable.
			// A grant is root-only, and root can write the same award straight to storage.
			ensure_root(origin)?;

			Self::do_grant_nft_claim_credit(
				claimant,
				attester,
				game_index,
				round,
				attester_position,
			)
		}
	}
}

impl<T: Config> Pallet<T> {
	/// The [`AwardedNftClaimCredits`] slot of the credit the co-player at `attester_position`
	/// of a group awards in `round`.
	///
	/// `attester_position` is the attester's own place in the group, so both award paths read
	/// the same number off the same list: `report` takes the reporter's place once per
	/// round, the reporter being the attester there, and the backfill each co-member's as it
	/// walks the group. The attestee's own place goes unused.
	///
	/// Spacing slots by the configured `MaxGroupSize` rather than the game's keeps the
	/// mapping independent of the group size, below `max_credit_slots`, which the
	/// `integrity_test` holds to [`AwardedCredits::CAPACITY`].
	pub fn credit_slot(round: RoundIndex, attester_position: AttesterPosition) -> CreditSlot {
		CreditSlot::from(round)
			.saturating_mul(T::MaxGroupSize::get())
			.saturating_add(attester_position)
	}

	/// The number of credit slots any game of this runtime can use.
	pub(crate) fn max_credit_slots() -> u32 {
		T::MaxRounds::get().saturating_mul(T::MaxGroupSize::get())
	}

	/// Compute the NFT claim credit for a successful report.
	///
	/// Blake2 256 hash of
	/// ```txt
	/// "polkadot-pop-game" ++ game index ++ attester ++ attestee ++ round
	/// ```
	/// - `game_index`: unsigned 32bit.
	/// - `attester` and `attestee`:
	///   - if an account-based player: 0 ++ account id.
	///   - if a person-based player: 1 ++ person id.
	/// - `round`: unsigned 8bit.
	pub fn compute_nft_claim_credit(
		game_index: GameIdx,
		round: u8,
		attester: &AccountOrPerson<T::AccountId>,
		attestee: &AccountOrPerson<T::AccountId>,
	) -> NftClaimCredit {
		(b"polkadot-pop-game", game_index, round, attester, attestee)
			.using_encoded(sp_io::hashing::blake2_256)
	}

	/// Compute the Merkle leaf committing to `credit` being owned by `claimant`.
	///
	/// Blake2 256 hash of the SCALE encoding of `(claimant, credit)`.
	///
	/// Hashed by the shared [`indiv_support::credit_trees::credit_leaf`], so the claim chain
	/// recomputes the leaf exactly as it was committed. The claimant is bound in because a
	/// credit is itself a hash and does not say who may mint. Nothing else is added: the
	/// credit already commits to the game index, the round and both players.
	pub fn compute_nft_claim_credit_leaf(
		claimant: &AccountOrPerson<T::AccountId>,
		credit: &NftClaimCredit,
	) -> NftClaimCreditLeaf {
		indiv_support::credit_trees::credit_leaf(claimant, credit)
	}

	/// Award `credit`, the credit of `claimant`'s `credit_slot` in this game, and record it in
	/// the current block's [`NftClaimCreditAwards`].
	///
	/// A credit is awarded once and only ever contributes one leaf: a slot already set in
	/// [`AwardedNftClaimCredits`] awards nothing. Both call sites can reach the same slot —
	/// `report` awards a `Person` vote's credit immediately and
	/// `award_attendance_credits` backfills every co-member's credit once the
	/// attendee is finalised — and awarding one twice would let the claimant mint twice from
	/// it, or leave a tree that can never be fully claimed when both leaves land in the same
	/// block.
	///
	/// The award is recorded before the slot is marked, so a full block marks nothing: leaving
	/// the slot clear is what lets a later block award the credit. The root info is written
	/// after the award for the same reason, so that a skipped award cannot leave the info set
	/// over a block with no awards.
	///
	/// Returns the number of awards recorded, which is one for a fresh credit and zero for
	/// one already awarded or one a full block skipped. Callers reserving block capacity use
	/// it to debit what was really spent.
	pub fn award_nft_claim_credit(
		game_index: GameIdx,
		claimant: &AccountOrPerson<T::AccountId>,
		credit: NftClaimCredit,
		credit_slot: CreditSlot,
		award_time: u32,
	) -> u32 {
		if !AwardedCredits::within_capacity(credit_slot) {
			// `credit_slot` is below `max_received_votes()`, which the `integrity_test`
			// holds to the mask's capacity, so this cannot be reached.
			defensive!("indiv-pallet-game: credit slot must fit the awarded credit mask");
			return 0;
		}
		if AwardedNftClaimCredits::<T>::get(game_index, claimant).contains(credit_slot) {
			return 0;
		}

		let award = NftClaimCreditAward { claimant: claimant.clone(), credit };
		let block = frame_system::Pallet::<T>::block_number();
		let leaf_index = NftClaimCreditAwards::<T>::decode_len(block).unwrap_or(0) as u32;

		if NftClaimCreditAwards::<T>::try_append(block, award).is_err() {
			// The `integrity_test` holds `MaxCreditsPerBlock` to what a block of `report`s
			// awards, and the backfill defers a player whose worst case does not fit, so a
			// full block means the bound is below what the runtime can award. Every credit
			// past it is lost: committed to no root and unmintable.
			defensive!("indiv-pallet-game: block must have room for the awarded credit");
			return 0;
		}

		if leaf_index == 0 {
			PendingNftClaimCreditRootInfo::<T>::put(NftClaimCreditRootInfo {
				game_index,
				timestamp: award_time,
			});
		} else if PendingNftClaimCreditRootInfo::<T>::get()
			.is_some_and(|info| info.game_index != game_index)
		{
			// Only one game runs at a time and a game's credits are all awarded while it
			// is the current one, so every leaf of a block belongs to the game the tree
			// info names. A mismatch would label the tree with the wrong game.
			defensive!("indiv-pallet-game: a block's credits must all belong to one game");
		}

		AwardedNftClaimCredits::<T>::mutate(game_index, claimant, |awarded| {
			awarded.insert(credit_slot)
		});
		Self::note_credit_block(claimant, block);
		Self::deposit_event(Event::<T>::NftClaimCreditAwarded {
			claimant: claimant.clone(),
			credit,
			leaf_index,
		});
		1
	}

	/// Record `award_block` in `claimant`'s [`NftClaimCreditBlocks`] index.
	///
	/// The block a credit is awarded in is the one whose tree will hold it, since
	/// [`Self::build_credit_tree`] keys each tree by the block its leaves were awarded in.
	///
	/// Only the last entry is compared, which is enough to keep the list free of repeats:
	/// entries are appended in block order, so a block already noted for this claimant can
	/// only be the last one.
	fn note_credit_block(claimant: &AccountOrPerson<T::AccountId>, award_block: BlockNumberFor<T>) {
		NftClaimCreditBlocks::<T>::mutate(claimant, |blocks| {
			if blocks.last() == Some(&award_block) {
				return;
			}
			if blocks.try_push(award_block).is_err() {
				blocks.remove(0);
				let _ = blocks
					.try_push(award_block)
					.defensive_proof("credit block list must hold one more block after pop");
			}
		});
	}

	/// The number of further NFT claim credits the current block can award.
	pub fn remaining_credit_capacity() -> u32 {
		let block = frame_system::Pallet::<T>::block_number();
		T::MaxCreditsPerBlock::get()
			.saturating_sub(NftClaimCreditAwards::<T>::decode_len(block).unwrap_or(0) as u32)
	}

	/// Build the Merkle tree over the credits awarded in the block before `now` and record
	/// its root.
	///
	/// Runs in `on_initialize`, so the awards of block `now - 1` are complete: nothing can
	/// be appended to them after that block ended. A block that awarded no credit is skipped
	/// entirely and gets no [`NftClaimCreditRoots`] entry.
	///
	/// Each block's tree stands alone. It is built once, from a complete leaf set, so no
	/// root grows over time and no inclusion proof can go stale. The awards it was built
	/// over stay for the retained window, so a claim can be proven from state; the root
	/// itself is never removed.
	///
	/// Every root recorded is also queued for delivery to the NFT claims chain, which the
	/// offchain worker then ships (see [`Pallet::send_credit_trees`]).
	///
	/// Emptiness is decided on [`PendingNftClaimCreditRootInfo`], not on the block's awards,
	/// even though either would answer it. A read's proof size is charged at the key's
	/// `max_size`, so touching the awards would bill every block of the chain for a full
	/// `MaxCreditsPerBlock` of them; the info value is a few bytes. Only a block that really
	/// has awards to commit to pays for them.
	pub fn build_credit_tree(now: BlockNumberFor<T>) -> Weight {
		let Some(NftClaimCreditRootInfo { game_index, timestamp }) =
			PendingNftClaimCreditRootInfo::<T>::get()
		else {
			return <T as Config>::WeightInfo::build_credit_tree_empty();
		};
		PendingNftClaimCreditRootInfo::<T>::kill();

		let block = now.saturating_sub(One::one());
		let awards = NftClaimCreditAwards::<T>::get(block);
		let leaf_count = awards.len() as u32;
		if leaf_count == 0 {
			defensive!("indiv-pallet-game: root info must be set with at least one award");
			return <T as Config>::WeightInfo::build_credit_tree(leaf_count);
		}

		let leaves = Self::nft_claim_credit_leaves(&awards);
		let root = binary_merkle_tree::merkle_root::<BlakeTwo256, _>(leaves).into();
		let credit_root = NftClaimCreditTree { game_index, root, leaf_count, timestamp };
		NftClaimCreditRoots::<T>::insert(block, credit_root);
		Self::retain_credit_awards(block);
		Self::deposit_event(Event::<T>::NftClaimCreditRootRecorded { block, credit_root });
		Self::queue_credit_tree_delivery(block);

		<T as Config>::WeightInfo::build_credit_tree(leaf_count)
	}

	/// Queue the credit tree of `block` for delivery to the NFT claims chain, under the next
	/// delivery sequence number.
	///
	/// A full queue means delivery has been failing for `MaxQueuedCreditTrees` trees. The tree
	/// stays in [`NftClaimCreditRoots`], so [`Self::replay_credit_trees`] can still deliver it.
	/// It consumes no sequence number, so the claims chain never reports a gap for a message
	/// that was never sent.
	pub fn queue_credit_tree_delivery(block: BlockNumberFor<T>) {
		let sequence = NextCreditTreeSequence::<T>::get();
		let mut queued = CreditTreeDeliveryQueue::<T>::get();

		if queued.try_push((sequence, block)).is_err() {
			log::error!(
				target: LOG_TARGET,
				"Credit tree delivery queue is full, the tree of block {block:?} needs a replay",
			);
			Self::deposit_event(Event::<T>::CreditTreeDeliveryDropped { block });
			return;
		}

		CreditTreeDeliveryQueue::<T>::put(queued);
		NextCreditTreeSequence::<T>::put(sequence.saturating_add(1));
	}

	/// Validates a `send_credit_trees` transaction.
	///
	/// The call only ever comes from this pallet's own offchain worker, so it is restricted to
	/// local and in-block sources. `first_sequence` is held to the front of the queue, which
	/// orders retries: a lower sequence has already been delivered and is `Stale`, a higher
	/// one is `Future` until the queue catches up.
	pub fn authorize_send_credit_trees(
		source: TransactionSource,
		first_sequence: &TreeSequence,
	) -> Result<(ValidTransaction, Weight), TransactionValidityError> {
		if !matches!(source, TransactionSource::InBlock | TransactionSource::Local) {
			return Err(AuthorizeInvalidity::TransactionNotLocal.into());
		}

		let Some((queued_sequence, _)) = CreditTreeDeliveryQueue::<T>::get().first().copied()
		else {
			return Err(AuthorizeInvalidity::NoQueuedCreditTrees.into());
		};
		if *first_sequence < queued_sequence {
			return Err(InvalidTransaction::Stale.into());
		}
		if *first_sequence > queued_sequence {
			return Err(InvalidTransaction::Future.into());
		}

		// A finite longevity lets a stranded retry self-evict rather than linger. Propagation
		// is off because peers validate gossiped transactions with a source of `External`,
		// which this call rejects.
		let validity =
			ValidTransaction::with_tag_prefix("game:send-credit-trees")
				.and_provides(queued_sequence)
				.priority(tx_priority::BACKGROUND_PROGRESS.saturating_add(
					frame_system::Pallet::<T>::block_number().saturated_into::<u64>(),
				))
				.longevity(CREDIT_TREE_TX_LONGEVITY)
				.propagate(false)
				.build()
				.expect("tag prefix is not empty; qed");

		Ok((validity, Weight::zero()))
	}

	/// Reads the credit tree of every block in `blocks`, dropping the blocks that have none.
	///
	/// A sequenced block without a tree is inconsistent state, not user error: nothing removes
	/// a tree while its delivery is outstanding. It is logged and reported with
	/// [`Event::CreditTreeDeliverySkipped`], because its sequence is spent either way.
	/// A block from [`Pallet::replay_credit_trees`] carries no sequence and is only logged,
	/// since there a block without a tree is the caller's own choice of argument.
	fn resolve_credit_trees(
		blocks: impl Iterator<Item = (Option<TreeSequence>, BlockNumberFor<T>)>,
	) -> Vec<CreditTreeDelivery> {
		blocks
			.filter_map(|(sequence, block)| {
				let Some(tree) = NftClaimCreditRoots::<T>::get(block) else {
					log::error!(
						target: LOG_TARGET,
						"No credit tree for block {block:?}, skipping its delivery",
					);
					if let Some(sequence) = sequence {
						Self::deposit_event(Event::<T>::CreditTreeDeliverySkipped {
							sequence,
							block,
						});
					}
					return None;
				};
				Some(CreditTreeDelivery {
					sequence,
					block: block.saturated_into::<AwardBlock>(),
					tree,
				})
			})
			.collect::<Vec<_>>()
	}

	/// Sends `updates` to the NFT claims chain in a single XCM message.
	fn send_credit_tree_batch(updates: Vec<CreditTreeDelivery>) -> DispatchResult {
		let trees = BoundedVec::try_from(updates).map_err(|_| {
			defensive!("credit tree batch must fit MaxCreditTreesPerMessage");
			Error::<T>::ExceedsClaimsChannelCapacity
		})?;
		let batch = CreditTreeBatch::<T> { source_time: T::UnixTime::now().as_secs(), trees };

		let call =
			(T::NftClaimsPalletIndex::get(), NftClaimsCall::<T>::ReceiveCreditTrees { batch })
				.encode();
		let destination = Location::new(1, [Parachain(T::NftClaimsParaId::get().into())]);

		send_xcm::<T::XcmRouter>(destination, Self::credit_tree_xcm(call))
			.map_err(|_| Error::<T>::CreditTreeXcmFailed)?;

		Ok(())
	}

	/// The XCM message a credit tree batch travels in.
	fn credit_tree_xcm(encoded_call: Vec<u8>) -> Xcm<()> {
		Xcm(vec![
			UnpaidExecution { weight_limit: WeightLimit::Unlimited, check_origin: None },
			Transact {
				origin_kind: OriginKind::Native,
				call: encoded_call.into(),
				fallback_max_weight: None,
			},
		])
	}

	/// The encoded size of the message carrying `trees` credit trees: a `VersionedXcm<()>` around
	/// the `Transact`, which is what a router compares against the channel's `max_message_size`.
	///
	/// Every field of a delivery is fixed-size, so the batch grows by exactly one
	/// `CreditTreeDelivery` per tree, plus the two compact length prefixes it sits behind: the
	/// batch's tree vector and the `Transact` call's own bytes.
	fn credit_tree_xcm_size(trees: u32) -> usize {
		let empty = CreditTreeBatch::<T> { source_time: 0, trees: BoundedVec::default() };
		let empty_call =
			(u8::MAX, NftClaimsCall::<T>::ReceiveCreditTrees { batch: empty }).encode();

		// The call once the batch holds `trees` deliveries: the empty vector's length prefix gives
		// way to the one for `trees`.
		let call_len = empty_call.len() - Compact(0u32).encoded_size() +
			Compact(trees).encoded_size() +
			trees as usize * CreditTreeDelivery::max_encoded_len();
		// The XCM around the call, whose size is measured with the empty call in it and taken
		// apart again, since only the router knows what the envelope itself encodes to.
		let envelope = VersionedXcm::<()>::from(Self::credit_tree_xcm(empty_call.clone()))
			.encode()
			.len() - empty_call.encoded_size();

		envelope + Compact(call_len as u32).encoded_size() + call_len
	}

	/// The number of credit trees that fit one XCM message to the NFT claims chain, or `None`
	/// when there is no channel to it or not even one tree fits.
	pub fn max_credit_trees_per_message() -> Option<u32> {
		let max_message_size = T::ChannelInfo::get_channel_info(T::NftClaimsParaId::get())
			.map(|info| info.max_message_size as usize)?
			.saturating_sub(CREDIT_TREE_ROUTER_HEADROOM);
		let available = max_message_size.saturating_sub(Self::credit_tree_xcm_size(0));

		// The compact prefixes grow with the batch, so dividing the room by a delivery's size can
		// land one tree over what the channel takes. At most one step walks that back.
		let mut count = (available / CreditTreeDelivery::max_encoded_len()) as u32;
		count = count.min(T::MaxCreditTreesPerMessage::get());
		while count > 0 && Self::credit_tree_xcm_size(count) > max_message_size {
			count -= 1;
		}

		(count > 0).then_some(count)
	}

	/// The per-message room the claims channel needs to carry `trees` credit trees and no more,
	/// which inverts [`Pallet::max_credit_trees_per_message`].
	///
	/// Only tests and benchmarks size a channel, and they need the same headroom the capacity
	/// keeps back for the router, or the message they set the channel up for does not fit it.
	pub fn credit_tree_channel_size(trees: u32) -> u32 {
		(Self::credit_tree_xcm_size(trees) + CREDIT_TREE_ROUTER_HEADROOM) as u32
	}

	/// Submits a [`Pallet::send_credit_trees`] transaction for the queued trees, if any.
	///
	/// Runs every block, so a delivery that failed is retried on the next one. Submission
	/// failures are expected: within a retry window every attempt is byte-identical and the
	/// transaction pool deduplicates them. A streak of them is not, so every
	/// [`CREDIT_TREE_STALL_WARN_PERIOD`] blocks the failure is warned about instead, a stalled
	/// delivery being otherwise visible only as a queue that stops draining.
	pub(crate) fn submit_credit_tree_delivery(block_number: BlockNumberFor<T>) {
		let Some((first_sequence, _)) = CreditTreeDeliveryQueue::<T>::get().first().copied() else {
			return;
		};

		let call = Call::<T>::send_credit_trees {
			first_sequence,
			discriminator: block_number / CREDIT_TREE_RETRY_WINDOW.into(),
		};
		let tx =
			<T as CreateAuthorizedTransaction<Call<T>>>::create_authorized_transaction(call.into());
		if SubmitTransaction::<T, Call<T>>::submit_transaction(tx).is_ok() {
			return;
		}

		if (block_number % CREDIT_TREE_STALL_WARN_PERIOD.into()).is_zero() {
			log::warn!(
				target: LOG_TARGET,
				"offchain worker: `send_credit_trees` repeatedly rejected by the \
				 transaction pool, possible stall",
			);
		} else {
			log::debug!(
				target: LOG_TARGET,
				"offchain worker: failed to submit `send_credit_trees`",
			);
		}
	}

	/// Note `block` as the newest award block whose [`NftClaimCreditAwards`] are retained,
	/// dropping the oldest one when that exceeds [`Config::MaxRetainedAwardBlocks`].
	///
	/// Only one block can drop out per call, one being added, so the ring is walked one entry
	/// at a time and the map holds at most the bound.
	fn retain_credit_awards(block: BlockNumberFor<T>) {
		NftClaimCreditAwardBlocks::<T>::mutate(|blocks| {
			if blocks.try_push(block).is_ok() {
				return;
			}
			if blocks.is_empty() {
				// `MaxRetainedAwardBlocks` is asserted non-zero by the `integrity_test`, so
				// an empty ring always has room.
				defensive!("indiv-pallet-game: award block ring must hold one block");
				return;
			}
			let dropped = blocks.remove(0);
			NftClaimCreditAwards::<T>::remove(dropped);
			let _ = blocks
				.try_push(block)
				.defensive_proof("award block ring must hold one more block after pop");
		});
	}

	/// The Merkle leaves `awards` commit to, in award order.
	fn nft_claim_credit_leaves(
		awards: &[NftClaimCreditAward<T::AccountId>],
	) -> Vec<NftClaimCreditLeaf> {
		awards
			.iter()
			.map(|award| Self::compute_nft_claim_credit_leaf(&award.claimant, &award.credit))
			.collect::<Vec<_>>()
	}

	/// The inclusion proofs of every NFT claim credit `claimant` was awarded in `award_block`,
	/// in leaf order, which is what Asset Hub verifies a mint against.
	///
	/// Reads the block's awards from [`NftClaimCreditAwards`], so a claimant needs nothing
	/// but their own identity: neither the block's other awards, nor the leaf format, nor the
	/// tree layout. Empty if the block awarded `claimant` nothing.
	///
	/// Only blocks inside the [`Config::MaxRetainedAwardBlocks`] window can be served this
	/// way. An older block gives [`NftClaimCreditProofError::AwardsPruned`], its root being
	/// kept but its awards not, and has to go through
	/// [`Self::nft_claim_credit_proof_from_awards`] instead.
	///
	/// Exposed through the runtime API rather than as a call: nothing is written, and a whole
	/// block's awards in an extrinsic's proof would be paid for by every other extrinsic in
	/// the block.
	///
	/// A call with at least one proof derives the leaves and builds the block's tree once.
	/// [`Config::MaxCreditsPerBlock`] bounds the tree hash count, regardless of the proof count.
	/// Each proof adds only its own sibling hashes.
	pub fn nft_claim_credit_proofs(
		award_block: BlockNumberFor<T>,
		claimant: &AccountOrPerson<T::AccountId>,
	) -> Result<Vec<NftClaimCreditProof>, NftClaimCreditProofError> {
		let recorded = NftClaimCreditRoots::<T>::get(award_block)
			.ok_or(NftClaimCreditProofError::UnknownAwardBlock)?;
		// A block with a root awarded at least one credit, so no awards means they were
		// pruned rather than that the block never had any.
		let awards = NftClaimCreditAwards::<T>::get(award_block);
		if awards.is_empty() {
			return Err(NftClaimCreditProofError::AwardsPruned);
		}

		let claimed = awards
			.iter()
			.enumerate()
			.filter(|(_, award)| &award.claimant == claimant)
			.map(|(leaf_index, award)| (leaf_index as u32, award.credit))
			.collect::<Vec<_>>();
		if claimed.is_empty() {
			return Ok(Vec::new());
		}

		let leaves = Self::nft_claim_credit_leaves(&awards);
		Self::credit_proofs(&recorded, &leaves, &claimed)
	}

	/// Build the inclusion proof of the credit at `leaf_index` against the
	/// [`NftClaimCreditRoots`] entry of `award_block`, from `awards` given by the caller.
	///
	/// The fallback for a block whose awards are no longer retained: `awards` are all the
	/// credits `award_block` awarded, in award order, which a wallet or an indexer rebuilds
	/// from the block's `NftClaimCreditAwarded` events. Prefer
	/// [`Self::nft_claim_credit_proofs`], which needs no such input.
	///
	/// The recomputed root is checked against the recorded one, so awards that are incomplete
	/// or out of order give [`NftClaimCreditProofError::RootMismatch`] here instead of a proof
	/// Asset Hub silently rejects.
	pub fn nft_claim_credit_proof_from_awards(
		award_block: BlockNumberFor<T>,
		awards: Vec<NftClaimCreditAward<T::AccountId>>,
		leaf_index: u32,
	) -> Result<NftClaimCreditProof, NftClaimCreditProofError> {
		let recorded = NftClaimCreditRoots::<T>::get(award_block)
			.ok_or(NftClaimCreditProofError::UnknownAwardBlock)?;
		if awards.len() as u32 != recorded.leaf_count {
			return Err(NftClaimCreditProofError::LeafCountMismatch {
				expected: recorded.leaf_count,
			});
		}
		let credit = awards
			.get(leaf_index as usize)
			.ok_or(NftClaimCreditProofError::LeafIndexOutOfBounds)?
			.credit;

		let leaves = Self::nft_claim_credit_leaves(&awards);
		Self::credit_proofs(&recorded, &leaves, &[(leaf_index, credit)])?
			.into_iter()
			.next()
			.defensive_ok_or(NftClaimCreditProofError::LeafIndexOutOfBounds)
	}

	/// The inclusion proofs of `claimed` in `leaves`, checked against `recorded`.
	///
	/// `claimed` pairs a leaf index with the credit that leaf commits to, and the proofs come
	/// back in that order. `leaves` must be the block's complete leaf set in award order; the
	/// root check is what establishes that it is. One tree serves every index.
	fn credit_proofs(
		recorded: &NftClaimCreditTree,
		leaves: &[NftClaimCreditLeaf],
		claimed: &[(u32, NftClaimCredit)],
	) -> Result<Vec<NftClaimCreditProof>, NftClaimCreditProofError> {
		let claimed = claimed
			.iter()
			.map(|(leaf_index, credit)| {
				if (*leaf_index as usize) < leaves.len() {
					Ok((*leaf_index, *credit))
				} else {
					Err(NftClaimCreditProofError::LeafIndexOutOfBounds)
				}
			})
			.collect::<Result<Vec<_>, _>>()?;

		let (root, proofs) =
			Self::credit_tree_proofs(leaves, claimed.iter().map(|(leaf_index, _)| *leaf_index));
		if root != recorded.root {
			return Err(NftClaimCreditProofError::RootMismatch);
		}

		Ok(claimed
			.into_iter()
			.zip(proofs)
			.map(|((leaf_index, credit), proof)| NftClaimCreditProof { credit, leaf_index, proof })
			.collect::<Vec<_>>())
	}

	/// The root of the tree over `leaves` and, for each index in `leaf_indices`, the sibling
	/// hashes that rehash its leaf up to that root, bottom layer first.
	///
	/// One pass over the layers serves every index at once. The layout is the one
	/// [`binary_merkle_tree`] builds and the claim chain's `verify_proof` rehashes along: a
	/// layer is hashed in pairs, a trailing odd node moves up unchanged, and the last node left
	/// is the root. An empty leaf set gives the zero root, as `merkle_root` does.
	fn credit_tree_proofs(
		leaves: &[NftClaimCreditLeaf],
		leaf_indices: impl Iterator<Item = u32>,
	) -> (CreditProofNode, Vec<Vec<CreditProofNode>>) {
		let mut layer = leaves
			.iter()
			.map(|leaf| sp_io::hashing::blake2_256(leaf.as_ref()))
			.collect::<Vec<_>>();
		let mut positions = leaf_indices.collect::<Vec<_>>();
		let mut proofs = vec![Vec::new(); positions.len()];

		while layer.len() > 1 {
			for (proof, position) in proofs.iter_mut().zip(&positions) {
				// A trailing odd node is alone in its pair, so it contributes no sibling.
				if let Some(sibling) = layer.get((position ^ 1) as usize) {
					proof.push(CreditProofNode(*sibling));
				}
			}

			layer = layer
				.chunks(2)
				.map(|pair| match pair {
					[left, right] => {
						let mut buf = [0u8; 64];
						buf[..32].copy_from_slice(left);
						buf[32..].copy_from_slice(right);
						sp_io::hashing::blake2_256(&buf)
					},
					_ => pair.first().copied().unwrap_or_default(),
				})
				.collect::<Vec<_>>();
			for position in &mut positions {
				*position /= 2;
			}
		}

		(CreditProofNode(layer.first().copied().unwrap_or_default()), proofs)
	}

	/// The NFT claim credit roots `claimant` has at least one credit under, in ascending
	/// block order.
	///
	/// Resolves [`NftClaimCreditBlocks`] against [`NftClaimCreditRoots`], so a wallet learns
	/// in one query which award blocks to ask [`Self::nft_claim_credit_proofs`] about and what
	/// root each of them commits to. The block a credit was awarded in gets its root only in
	/// the next block, so the newest award block is left out until then.
	pub fn nft_claim_credit_roots(
		claimant: &AccountOrPerson<T::AccountId>,
	) -> Vec<(BlockNumberFor<T>, NftClaimCreditTree)> {
		NftClaimCreditBlocks::<T>::get(claimant)
			.into_iter()
			.filter_map(|block| NftClaimCreditRoots::<T>::get(block).map(|root| (block, root)))
			.collect::<Vec<_>>()
	}

	/// Award every NFT claim credit a freshly-attended `attendee` is entitled to for the
	/// current game.
	///
	/// An attendee earns one credit per other member of their group, in every round
	/// they played — irrespective of whether each co-member submitted a report or
	/// what they voted. During the reporting phase the `report` extrinsic awards
	/// these on the fly, but the early-attendance optimisation lets losing players
	/// skip reporting entirely, so attendees can end up missing credits from
	/// non-reporting co-members. This helper closes that gap by walking, for each
	/// round the attendee played in, every other member of their group and
	/// inserting the corresponding credit entry.
	///
	/// Credits already awarded by a real `Person` report are walked again here.
	/// [`Self::award_nft_claim_credit`] leaves those entries untouched, so each credit
	/// keeps its first award block and is committed to exactly one Merkle root.
	///
	/// The caller must have checked that [`Self::remaining_credit_capacity`] covers
	/// [`Self::max_attestations`], the most this can award for one attendee. Returns how
	/// many credits were really awarded, which is fewer whenever a credit was already
	/// awarded during reporting.
	pub(crate) fn award_attendance_credits(
		game_index: GameIdx,
		rounds: u8,
		max_group_size: u32,
		player_count: u32,
		attendee: &AccountOrPerson<T::AccountId>,
		award_time: u32,
	) -> u32 {
		let Some(attendee_indices) = PlayerToIndex::<T>::get(attendee) else {
			// Unregistered attendee (should not happen for `attendance == true`,
			// since `determine_attendance` short-circuits non-registered players).
			defensive!("indiv-pallet-game: attended player must have round indices");
			return 0;
		};

		let mut awarded = 0;

		let groups_setting = GroupsSetting { max_per_group: max_group_size, player_count };

		for round in 0..rounds {
			let Some(&attendee_index) = attendee_indices.get(round as usize) else {
				defensive!("indiv-pallet-game: attendee should have an index for each round");
				continue;
			};

			let group_index = groups_setting.group_index_from_player_index(attendee_index);
			// Enumerating the whole group before dropping the attendee is what makes the
			// position the attester's own place in it, the same one `report` reads.
			let co_members = groups_setting
				.group_members(group_index)
				.enumerate()
				.filter(|&(_, index)| index != attendee_index);

			for (attester_position, co_member_index) in co_members {
				let Some(co_member) = IndexToPlayer::<T>::get((round, co_member_index)) else {
					defensive!(
						"indiv-pallet-game: index should map to a player when awarding credits"
					);
					continue;
				};
				let credit =
					Self::compute_nft_claim_credit(game_index, round, &co_member, attendee);
				let credit_slot = Self::credit_slot(round, attester_position as AttesterPosition);
				awarded = awarded.saturating_add(Self::award_nft_claim_credit(
					game_index,
					attendee,
					credit,
					credit_slot,
					award_time,
				));
			}
		}

		awarded
	}

	/// Delivers the queued credit trees that fit one XCM message, as
	/// [`Pallet::send_credit_trees`] does once its origin is checked.
	///
	/// A message that cannot be built or sent leaves the queue untouched and reports
	/// `CreditTreeSendFailed`, so the next offchain-worker cycle retries the same front.
	pub(crate) fn do_send_credit_trees() -> DispatchResultWithPostInfo {
		let queued = CreditTreeDeliveryQueue::<T>::get();
		debug_assert!(!queued.is_empty(), "authorize should have rejected: nothing queued");

		let Some(message_capacity) = Self::max_credit_trees_per_message() else {
			log::warn!(
				target: LOG_TARGET,
				"No channel capacity to the NFT claims chain, retrying next offchain worker cycle",
			);
			Self::deposit_event(Event::<T>::CreditTreeSendFailed);
			return Ok(Some(<T as Config>::WeightInfo::send_credit_trees(0)).into());
		};

		let taken = (message_capacity as usize).min(queued.len());
		let updates = Self::resolve_credit_trees(
			queued[..taken].iter().map(|(sequence, block)| (Some(*sequence), *block)),
		);

		// The blocks the message delivers, in sequence order, which the event carries so that a
		// gap the claims chain reports can be turned back into the block a replay needs.
		// Truncation is unreachable: `updates` came out of `queued[..taken]`, which
		// `message_capacity` already held to this very bound.
		let sent = BoundedVec::<_, T::MaxCreditTreesPerMessage>::truncate_from(
			updates.iter().map(|update| update.block).collect::<Vec<_>>(),
		);

		// An empty batch means every queued block has lost its tree in the meantime, so there
		// is nothing to send, but the queue entries still have to go.
		let count = updates.len() as u32;
		if count > 0 {
			if let Err(e) = Self::send_credit_tree_batch(updates) {
				log::warn!(
					target: LOG_TARGET,
					"Credit tree XCM failed: {e:?}, retrying next offchain worker cycle",
				);
				Self::deposit_event(Event::<T>::CreditTreeSendFailed);
				return Ok(Some(<T as Config>::WeightInfo::send_credit_trees(count)).into());
			}
		}

		CreditTreeDeliveryQueue::<T>::mutate(|queued| {
			queued.drain(..taken.min(queued.len()));
		});
		Self::deposit_event(Event::<T>::CreditTreesSent { trees: sent });

		Ok(Some(<T as Config>::WeightInfo::send_credit_trees(taken as u32)).into())
	}

	/// Resends the credit trees of `blocks`, as [`Pallet::replay_credit_trees`] does once its
	/// origin is checked.
	///
	/// The cooldown holds how often this may run.
	pub(crate) fn do_replay_credit_trees(
		blocks: BoundedVec<BlockNumberFor<T>, T::MaxCreditTreesPerMessage>,
	) -> DispatchResult {
		ensure!(!blocks.is_empty(), Error::<T>::NoBlocksToReplay);
		ensure!(blocks.windows(2).all(|w| w[0] < w[1]), Error::<T>::UnsortedReplayBlocks);

		let message_capacity =
			Self::max_credit_trees_per_message().ok_or(Error::<T>::ExceedsClaimsChannelCapacity)?;
		ensure!(blocks.len() as u32 <= message_capacity, Error::<T>::ExceedsClaimsChannelCapacity);

		let updates = Self::resolve_credit_trees(blocks.iter().map(|block| (None, *block)));
		ensure!(!updates.is_empty(), Error::<T>::NoCreditTreeForBlock);

		let now = T::UnixTime::now().as_secs();
		if let Some(last) = LastReplayTime::<T>::get() {
			ensure!(
				now.saturating_sub(last) >= T::ReplayCooldownSeconds::get(),
				Error::<T>::ReplayCooldownActive
			);
		}

		let count = updates.len() as u32;
		Self::send_credit_tree_batch(updates)?;
		LastReplayTime::<T>::put(now);

		Self::deposit_event(Event::<T>::CreditTreesReplayed { count });

		Ok(())
	}

	/// Awards `claimant` the credit `attester` would earn them in `round` of `game_index`, as
	/// [`Pallet::testnet_grant_nft_claim_credit`] does once its origin is checked.
	#[cfg(feature = "testnet")]
	pub(crate) fn do_grant_nft_claim_credit(
		claimant: AccountOrPerson<T::AccountId>,
		attester: AccountOrPerson<T::AccountId>,
		game_index: GameIdx,
		round: RoundIndex,
		attester_position: AttesterPosition,
	) -> DispatchResult {
		// The bounds a played game keeps the two within. Checked here so that the call
		// reports an out-of-range round or slot, rather than deriving a credit slot the
		// defensive guard in `award_nft_claim_credit` rejects.
		ensure!(
			u32::from(round) < T::MaxRounds::get() && attester_position < T::MaxGroupSize::get(),
			Error::<T>::CreditSlotOutOfBounds
		);

		let credit_slot = Self::credit_slot(round, attester_position);
		let credit = Self::compute_nft_claim_credit(game_index, round, &attester, &claimant);
		let award_time = T::UnixTime::now().as_secs() as u32;
		let awarded =
			Self::award_nft_claim_credit(game_index, &claimant, credit, credit_slot, award_time);
		ensure!(awarded > 0, Error::<T>::CreditNotAwarded);

		Ok(())
	}

	/// Asserts the configuration invariants of the NFT claim credits, as the pallet's
	/// `integrity_test` runs them.
	pub(crate) fn integrity_test_credits() {
		let e_max = indiv_pallet_game::Pallet::<T>::max_enactments().saturating_sub(1);

		// `report` is atomic: it cannot defer part of its credits to the next block, so its
		// up-to-`e_max` credits must fit a block that has awarded none, otherwise a report
		// loses credits however empty the block is. This floor holds for every configuration;
		// the block-wide one below needs real block limits and weights.
		assert!(
			e_max <= T::MaxCreditsPerBlock::get(),
			"one `report` awards up to {e_max} credits, more than `MaxCreditsPerBlock` ({max})",
			max = T::MaxCreditsPerBlock::get(),
		);

		// `MaxCreditsPerBlock` must also cover what a whole block awards, so that no `report`
		// awards a credit the block's awards entry has no room for. Such a credit is
		// committed to no Merkle root and stays unmintable, which
		// `Self::award_nft_claim_credit` can only report defensively.
		//
		// Only `report` awards within a block, the attendance backfill running in the
		// player-process phase and deferring a player rather than overflowing. Proof size is
		// what limits how many fit, binding long before `ref_time` because the awards entry is
		// charged at `max_size` on every one of them.
		//
		// Dividing that budget by a single worst-case report bounds every mix of them:
		// `report` pays a fixed base plus a per-credit term, so smaller reports award fewer
		// credits for the same proof. The enactment component stays zero for the same reason,
		// adding proof size without a credit. Metering is divided rather than the real PoV,
		// and rightly so: block building stops on the metered sum, while a PoV deduplicates
		// the keys its reports share and holds more of them.
		//
		// The benchmarking build is exempt, widening `MaxGroupSize` and `MaxRounds` to give
		// the linear regressions a range to fit over, which would size the bound for a runtime
		// that is never deployed.
		#[cfg(not(feature = "runtime-benchmarks"))]
		{
			let normal_proof = <T as frame_system::Config>::BlockWeights::get()
				.per_class
				.get(frame_support::dispatch::DispatchClass::Normal)
				.max_total
				.map_or(u64::MAX, |max_total| max_total.proof_size());
			let report_proof =
			<<T as indiv_pallet_game::Config>::WeightInfo as indiv_pallet_game::WeightInfo>::report(
				e_max, 0,
			)
			.proof_size();

			// Mocks leave the proof size at a fraction of `u64::MAX` and the weights at
			// placeholders, so nothing there bounds how many reports a block holds. Every
			// deployed runtime bounds a block's proof size to a few megabytes.
			if report_proof > 0 && normal_proof <= u32::MAX as u64 {
				let awarded = (normal_proof / report_proof).saturating_mul(e_max as u64);
				let bound = T::MaxCreditsPerBlock::get() as u64;
				assert!(
					awarded <= bound,
					"a block can award up to {awarded} credits, more than \
				`MaxCreditsPerBlock` ({bound}), so a `report` can award a lost credit",
				);
				// Only the floor is asserted. Slack above it is a cost, not a hazard, charged
				// to every report whether the awards are there or not. A ceiling is left out
				// because raising the bound raises that charge, which lowers `awarded`, so the
				// two together would leave a runtime hunting for a value satisfying both after
				// every weight regeneration.
				if bound > awarded.saturating_mul(2) {
					log::warn!(
						target: LOG_TARGET,
						"`MaxCreditsPerBlock` ({bound}) is more than twice what a block can \
						award ({awarded}); the spare capacity is charged to every report",
					);
				}
			} else {
				// The skip is the one thing that disables this check without failing, so it
				// says so: a deployed runtime satisfies both conditions.
				log::warn!(
					target: LOG_TARGET,
					"`MaxCreditsPerBlock` is unchecked against block limits: `report` charges \
					{report_proof} bytes of proof and `Normal` allows {normal_proof}",
				);
			}
		}

		// Every credit a claimant can earn in a game needs its own slot in
		// `AwardedNftClaimCredits`, otherwise the overflowing ones would be awarded twice,
		// once by `report` and once by the attendance backfill.
		assert!(
			Self::max_credit_slots() <= AwardedCredits::CAPACITY,
			"a game uses up to {slots} credit slots per claimant, more than the {capacity} \
		of `AwardedNftClaimCredits`",
			slots = Self::max_credit_slots(),
			capacity = AwardedCredits::CAPACITY,
		);

		let build_worst_case =
			<T as Config>::WeightInfo::build_credit_tree(T::MaxCreditsPerBlock::get());
		OcwWeightBudget::from_normal_max::<T>().assert_fits("build_credit_tree", build_worst_case);

		// A message must be fillable from a full queue, otherwise the queue's tail could
		// never be drained in one send.
		assert!(
			T::MaxCreditTreesPerMessage::get() > 0,
			"MaxCreditTreesPerMessage must be greater than zero",
		);
		assert!(
			T::MaxQueuedCreditTrees::get() >= T::MaxCreditTreesPerMessage::get(),
			"MaxQueuedCreditTrees ({queued}) must be >= MaxCreditTreesPerMessage \
		 ({per_message})",
			queued = T::MaxQueuedCreditTrees::get(),
			per_message = T::MaxCreditTreesPerMessage::get(),
		);

		// `send_credit_trees` is an offchain-worker transaction: a worst case above
		// `Normal.max_extrinsic` is dropped at the transaction-pool level, which stalls
		// credit tree delivery for good. `replay_credit_trees` is the manual repair for
		// exactly that case, so it has to stay submittable too.
		let max_trees = T::MaxCreditTreesPerMessage::get();
		let budget = OcwWeightBudget::from_normal_max::<T>();
		budget.assert_fits(
			"send_credit_trees",
			<T as Config>::WeightInfo::send_credit_trees(max_trees)
				.saturating_add(<T as Config>::WeightInfo::authorize_send_credit_trees()),
		);
		budget.assert_fits(
			"replay_credit_trees",
			<T as Config>::WeightInfo::replay_credit_trees(max_trees)
				.saturating_add(T::NftClaimsRemoteWeight::get().saturating_mul(max_trees.into())),
		);

		assert!(
			!T::ReplayCooldownSeconds::get().is_zero(),
			"`ReplayCooldownSeconds` must be at least one",
		);

		// A ring with no room retains no awards at all, leaving every claim to be rebuilt
		// from events, which is the fallback rather than the intended path.
		assert!(
			!T::MaxRetainedAwardBlocks::get().is_zero(),
			"`MaxRetainedAwardBlocks` must be at least one",
		);

		// Both bounds count award blocks and gain one per recorded root, so a queue wider than
		// the ring holds trees whose awards have already been pruned. Their delivery still
		// arrives, but the claims it carries are then provable from the block's events only.
		assert!(
			T::MaxRetainedAwardBlocks::get() >= T::MaxQueuedCreditTrees::get(),
			"MaxRetainedAwardBlocks ({retained}) must be >= MaxQueuedCreditTrees ({queued})",
			retained = T::MaxRetainedAwardBlocks::get(),
			queued = T::MaxQueuedCreditTrees::get(),
		);
	}
}

/// What the game triggers, and nothing more: the awards, the block's remaining room for them, and
/// the slots a game gives up when it ends or is cancelled.
///
/// Everything else about a credit — the tree, the delivery, the proofs — is this pallet's own and
/// the game never sees it.
impl<T: Config> AwardCredits<T::AccountId> for Pallet<T> {
	fn award_report_credit(
		game_index: GameIdx,
		round: RoundIndex,
		attester: &AccountOrPerson<T::AccountId>,
		attestee: &AccountOrPerson<T::AccountId>,
		attester_position: AttesterPosition,
		award_time: u32,
	) -> u32 {
		let credit = Self::compute_nft_claim_credit(game_index, round, attester, attestee);
		Self::award_nft_claim_credit(
			game_index,
			attestee,
			credit,
			Self::credit_slot(round, attester_position),
			award_time,
		)
	}

	fn award_attendance_credits(
		game_index: GameIdx,
		rounds: RoundIndex,
		max_group_size: u32,
		player_count: u32,
		attendee: &AccountOrPerson<T::AccountId>,
		award_time: u32,
	) -> u32 {
		Self::award_attendance_credits(
			game_index,
			rounds,
			max_group_size,
			player_count,
			attendee,
			award_time,
		)
	}

	fn remaining_capacity() -> u32 {
		Self::remaining_credit_capacity()
	}

	fn clear_game_credits(
		game_index: GameIdx,
		limit: u32,
		cursor: Option<&[u8]>,
	) -> Option<Vec<u8>> {
		AwardedNftClaimCredits::<T>::clear_prefix(game_index, limit, cursor).maybe_cursor
	}

	fn forget_player_credits(game_index: GameIdx, player: &AccountOrPerson<T::AccountId>) {
		AwardedNftClaimCredits::<T>::remove(game_index, player);
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn benchmark_award_every_slot(game_index: GameIdx, player: &AccountOrPerson<T::AccountId>) {
		AwardedNftClaimCredits::<T>::insert(game_index, player, AwardedCredits::FULL);
	}
}
