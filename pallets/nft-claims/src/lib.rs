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

//! # NFT Claims Pallet
//!
//! Holds the Merkle roots committing to the NFT claim credits the game pallet awards on the
//! People chain. A claim is verified against them, by an inclusion proof of the credit's leaf
//! under the root of the block it was awarded in.
//!
//! The commitments and the minting live in one pallet on purpose: the roots have exactly one
//! consumer, the claim, so there is nothing to be gained from splitting the two apart.
//!
//! ## Receiving the roots
//!
//! A root arrives in a `receive_credit_trees` batch that the game pallet sends over XCM, and is
//! stored under the People-chain block whose credits it commits to. Only that pallet's chain can
//! submit the call, through [`Config::EnsureGameChainOrigin`]. Receiving is idempotent: a tree
//! already held is left as it is, so a resend of a tree that did arrive changes nothing, and a
//! root can never be swapped out from under the proofs built against it.
//!
//! ## Claiming
//!
//! [`Pallet::claim`] mints the NFT of one credit. The claimant presents the credit, its leaf index
//! and the sibling hashes the game chain's `nft_claim_credit_proofs` returns, and this pallet
//! rehashes the leaf, `blake2_256(claimant ++ credit)`, up to the root it holds for the award
//! block. The claimant is the signer's own identity under the [`ClaimantKind`] the call names, so
//! a credit awarded to somebody else hashes to a leaf that is in no tree, and the root and leaf
//! count are the stored ones rather than anything the call carries.
//!
//! A claim is a signed transaction, whichever identity it is made under: the call names the
//! [`ClaimantKind`] and [`Config::EnsureClaimant`] resolves the person alias the signer is bound
//! to. The signer pays the transaction fee, in PGAS as any other call, so a failing claim always
//! costs its submitter.
//!
//! The NFT itself is a `pallet-scarcity` instance, minted with no storage deposit: the credit is
//! what bounds the state a claim creates, since the game chain awards a credit once and
//! [`ClaimedCredits`] spends it once. Scarcity purse keys hold one NFT each and take no
//! destination consent, so the call names the key to mint to rather than minting to the
//! claimant's own account.
//!
//! ## Collections and item selection
//!
//! The claimant names the collection a claim mints into, and a collection accepts claims only
//! once its owner has registered it through [`Pallet::set_collection_minter`], choosing an
//! [`ItemSelection`]: deposit-free minting inflates a collection's supply, so it takes the
//! owner's opt-in. A registration remains valid only while that owner holds the collection, so a
//! new owner has to register it again. The registration decides which of the collection's items a
//! claim mints:
//!
//! - [`ItemSelection::Contract`] asks the named contract, `mint(uint32 collection, bytes32
//!   entropy)`, with the credit as the only entropy, and mints the item index it returns. The
//!   current collection owner makes the bounded call and collateralizes its storage writes. Any
//!   failure fails the claim and leaves the credit unspent because the contract is how an owner
//!   gates their collection.
//! - [`ItemSelection::Random`] needs no contract: the item index is the credit modulo the
//!   collection's next item index. A claimant chooses which collection to claim into, but not the
//!   item within it: for a fixed collection and item set the credit maps to one item and the credit
//!   is fixed by game events before any claim. This assumes the owner defines the collection's
//!   items before opening it to claims, since the next item index is the modulus: adding items
//!   shifts which item a credit maps to, and deleting one leaves a hole a credit can still land on
//!   and fail.
//!
//! ## Missing trees
//!
//! Award blocks are not contiguous, since a block that awarded no credit has no tree, so a
//! missing tree cannot be spotted from the block numbers. Each tree of the live stream instead
//! carries a contiguous sequence number, and a batch whose first sequence is ahead of the one
//! expected means the trees in between never arrived: [`Event::CreditTreesMissing`] names them.
//! Recovering them is a `replay_credit_trees` call on the game pallet, naming the award blocks,
//! which anyone can submit. A resent tree carries no sequence number and leaves the tracking of
//! the live stream alone.
//!
//! The sequences a gap names are turned back into those award blocks on the game chain. Its
//! `CreditTreesSent` event lists the blocks one message delivered, in the order they go out, and
//! its `send_credit_trees` call names the sequence the run starts at, so walking the run pairs
//! each sequence with a block. The sequences left out of it are the ones the game pallet spent on
//! a tree whose root it had already dropped, named one by one by `CreditTreeDeliverySkipped`. No
//! replay recovers those: the root a proof would verify against no longer exists on either chain.

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

use frame_support::{
	dispatch::WithPostDispatchInfo,
	traits::{EnsureOrigin, EnsureOriginWithArg, Get},
	weights::Weight,
};
use indiv_support::{
	credit_trees::{
		credit_leaf, AwardBlock, CreditProofNode, NftClaimCredit, NftClaimCreditLeaf,
		NftClaimCreditTree, TreeSequence,
	},
	identity::AccountOrPerson,
	weight_budget::OcwWeightBudget,
};
use pallet_scarcity::{CollectionId, InspectCollection, InstanceId, ItemIndex, MintWithoutDeposit};
use sp_core::{H160, H256};
use sp_runtime::{traits::BlakeTwo256, DispatchError};

const LOG_TARGET: &str = "runtime::indiv-pallet-nft-claims";

/// Number of metadata entries a claim mints with, which `mint_hook_weight` prices per entry.
///
/// The weight annotation runs before the dispatch builds that metadata, so this cannot read the
/// vector's length. It is also the ceiling, because a dispatch may refund weight but never add
/// any, so a mint passing more entries than this undercharges and reports nothing. Raise it in
/// the same change that gives the mint metadata to pass.
const CLAIM_METADATA_PAIRS: u32 = 0;

/// Successful output of a collection's minter contract.
pub struct Selection {
	/// The item, within the collection the contract was asked about, the claim mints.
	pub item: ItemIndex,
	/// Weight the selection really consumed, refunded against
	/// [`CollectionSelector::max_weight`]. Must not exceed it.
	pub weight_consumed: Weight,
}

/// Failure of a collection's minter contract call.
pub struct SelectionError {
	/// What failed, which fails the claim.
	pub error: DispatchError,
	/// Weight the call really consumed before it failed, charged against
	/// [`CollectionSelector::max_weight`]. Must not exceed it.
	pub weight_consumed: Weight,
}

/// Runtime adapter calling a collection's minter contract as its current owner.
///
/// The contract exposes `mint(uint32 collection, bytes32 entropy) returns (uint32 item)` and uses
/// the claimed credit as its only entropy. The runtime limits execution and storage deposits.
pub trait CollectionSelector<AccountId> {
	/// Worst-case weight of one selection, reserved before dispatch.
	fn max_weight() -> Weight;

	/// Confirm `contract` can be registered as a minter, which is that code is deployed at the
	/// address.
	///
	/// Run once at registration to fail typos and not-yet-deployed contracts there, with a
	/// clear error, rather than on every claim. It is a courtesy, not a guarantee: nothing
	/// on-chain proves the code implements the minter interface, so [`Self::select`] still
	/// validates every call's outcome.
	fn validate(contract: H160) -> Result<(), DispatchError>;

	/// Ask `contract` as `owner` which of `collection`'s items the claim of `entropy` mints.
	///
	/// A failure reports the weight the call consumed before failing, so the claim charges it.
	fn select(
		owner: AccountId,
		contract: H160,
		collection: CollectionId,
		entropy: NftClaimCredit,
	) -> Result<Selection, SelectionError>;
}

/// What the benchmarks cannot set up themselves, because only the runtime knows how its NFT
/// backend is administered.
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper<AccountId> {
	/// Make `collection` exist owned by `owner`, with `item` defined in it, as the owner would
	/// have done before the first claim.
	fn prepare_collection(owner: &AccountId, collection: CollectionId, item: ItemIndex);

	/// Deploy a contract that the collection registration benchmark can validate.
	fn prepare_contract(owner: &AccountId) -> H160;
}

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use alloc::vec::Vec;
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;

		/// Origin check for the XCM messages carrying credit trees, which authenticates the
		/// chain the game pallet runs on.
		type EnsureGameChainOrigin: EnsureOrigin<Self::RuntimeOrigin>;

		/// Maximum number of credit trees accepted in one batch.
		///
		/// Must be at least the game pallet's `MaxCreditTreesPerMessage`, otherwise the batches
		/// it sends fail to decode and the trees in them are lost.
		#[pallet::constant]
		type MaxTreesPerMessage: Get<u32>;

		/// Origin check for a claim, resolving the signer to the identity a credit's leaf binds
		/// under the [`ClaimantKind`] the call names.
		///
		/// The origin has to stay a signed one, so that the signer pays the transaction's fee.
		/// Resolving [`ClaimantKind::Person`] means looking up the alias the signer is bound to,
		/// which fails for a signer that has none.
		type EnsureClaimant: EnsureOriginWithArg<
			Self::RuntimeOrigin,
			ClaimantKind,
			Success = AccountOrPerson<Self::AccountId>,
		>;

		/// The NFTs a claim mints, which is `pallet-scarcity`.
		///
		/// Minting is deposit-free: the credit is what bounds the state a claim creates, since a
		/// credit is awarded once by the game chain and this pallet spends it once. The inspect
		/// side gates [`Pallet::set_collection_minter`] on the collection's owner and sizes the
		/// [`ItemSelection::Random`] draw.
		type Nfts: MintWithoutDeposit<Self::AccountId> + InspectCollection<Self::AccountId>;

		/// Executes a registered [`ItemSelection::Contract`] minter as the current collection
		/// owner.
		///
		/// Every claim reserves `max_weight` and refunds it down to what the selection really
		/// consumed, whether the claim succeeds or fails. The runtime also limits the storage
		/// deposit the owner can pay for one call.
		type CollectionSelector: CollectionSelector<Self::AccountId>;

		/// Maximum number of sibling hashes an inclusion proof may carry.
		///
		/// A tree of `n` leaves needs `ceil(log2(n))` of them, so this must cover the game
		/// chain's `MaxCreditsPerBlock`: a lower bound leaves the tail of a large tree unclaimable.
		#[pallet::constant]
		type MaxProofNodes: Get<u32>;

		/// Setup the claim benchmark needs from the NFT backend.
		#[cfg(feature = "runtime-benchmarks")]
		type BenchmarkHelper: BenchmarkHelper<Self::AccountId>;
	}

	/// The Merkle commitment to the NFT claim credits awarded in one People-chain block, keyed
	/// by that block.
	#[pallet::storage]
	pub type CreditTrees<T: Config> =
		StorageMap<_, Twox64Concat, AwardBlock, NftClaimCreditTree, OptionQuery>;

	/// The sequence number of the next tree expected from the game pallet's live stream.
	///
	/// A batch starting above it means the trees in between were lost on the way.
	#[pallet::storage]
	pub type NextExpectedSequence<T: Config> = StorageValue<_, TreeSequence, ValueQuery>;

	/// The leaves of an award block's tree that have been claimed.
	/// A leaf commits to one claimant holding one credit, so it identifies the claim on its own.
	/// Entries are kept for good: dropping one would let that credit mint a second NFT.
	#[pallet::storage]
	pub type ClaimedCredits<T: Config> = StorageDoubleMap<
		_,
		Twox64Concat,
		AwardBlock,
		Identity,
		NftClaimCreditLeaf,
		(),
		OptionQuery,
	>;

	/// How many of an award block's leaves have been claimed.
	/// Against the tree's `leaf_count`, this tells whether anything is left to claim.
	#[pallet::storage]
	pub type ClaimedCounts<T: Config> = StorageMap<_, Twox64Concat, AwardBlock, u32, ValueQuery>;

	/// The collections whose owners accept claims, each bound to the registering owner and the
	/// [`ItemSelection`] deciding the item. A collection with no entry cannot be claimed into.
	#[pallet::storage]
	pub type CollectionMinters<T: Config> =
		StorageMap<_, Twox64Concat, CollectionId, CollectionMinter<T::AccountId>, OptionQuery>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// Credit trees were received and stored.
		CreditTreesReceived { count: u32, stored: u32 },
		/// Trees of the live stream never arrived. The game pallet's `CreditTreesSent` events
		/// resolve these sequences to the award blocks they were delivered under, and a
		/// `replay_credit_trees` naming those blocks recovers the trees.
		///
		/// A sequence that resolves to no block is one the game pallet spent on a tree it had
		/// already dropped the root of, and no replay brings it back.
		CreditTreesMissing { from_sequence: TreeSequence, to_sequence: TreeSequence },
		/// A tree was received for a block that already holds a different root. The stored
		/// root is kept, so the proofs built against it stay valid.
		CreditTreeConflict { block: AwardBlock },
		/// A credit awarded in `block` was claimed, minting `instance` of `collection`'s `item`
		/// to the purse key `owner`.
		CreditClaimed {
			block: AwardBlock,
			leaf: NftClaimCreditLeaf,
			collection: CollectionId,
			item: ItemIndex,
			owner: T::AccountId,
			instance: InstanceId,
		},
		/// Every credit committed to by `block`'s tree has now been claimed.
		TreeFullyClaimed { block: AwardBlock },
		/// `collection`'s owner registered it for claims with `selection`, or withdrew it with
		/// `None`.
		CollectionMinterSet { collection: CollectionId, selection: Option<ItemSelection> },
	}

	#[pallet::error]
	pub enum Error<T> {
		/// No tree is held for the award block, so nothing can be proven against it. The tree may
		/// still be on its way, or have been lost, in which case a `replay_credit_trees` on the
		/// game pallet delivers it.
		UnknownAwardBlock,
		/// The leaf index is not one of the tree's leaves.
		LeafIndexOutOfBounds,
		/// The credit has already been claimed and mints one NFT only.
		AlreadyClaimed,
		/// The proof does not rehash the credit's leaf to the tree's root, so the origin holds no
		/// such credit in that block.
		InvalidProof,
		/// The collection does not exist in Scarcity.
		UnknownCollection,
		/// Only the collection's owner may register or withdraw its minter.
		NotCollectionOwner,
		/// The collection's owner has not registered it for claims, so no claim can mint into
		/// it.
		CollectionNotRegistered,
		/// The collection has changed owners since registration, so its current owner must
		/// register it again.
		CollectionOwnerChanged,
		/// The collection has no item definitions for [`ItemSelection::Random`] to draw from.
		NoItems,
	}

	#[pallet::call(weight = <T as Config>::WeightInfo)]
	impl<T: Config> Pallet<T> {
		/// Stores the credit trees of a batch sent by the game pallet.
		///
		/// ## Origin
		/// Requires the game chain's XCM origin (`EnsureGameChainOrigin`).
		///
		/// ## Parameters
		/// - `batch`: The credit trees to store, in ascending block order.
		#[pallet::call_index(0)]
		#[pallet::weight(T::WeightInfo::receive_credit_trees(batch.trees.len() as u32))]
		pub fn receive_credit_trees(
			origin: OriginFor<T>,
			batch: CreditTreeBatch<T>,
		) -> DispatchResult {
			T::EnsureGameChainOrigin::ensure_origin(origin)?;

			let count = batch.trees.len() as u32;
			let mut stored = 0u32;

			for update in batch.trees.iter() {
				if update.tree.leaf_count == 0 || update.tree.root.0 == [0u8; 32] {
					// The game pallet only commits blocks that awarded at least one credit and
					// a Blake2 root of real leaves is never zero, so neither can be genuine. An
					// empty tree would be unclaimable and a zero root is not a commitment.
					log::error!(
						target: LOG_TARGET,
						"Invalid credit tree for block {}: root {:?}, leaf count {}",
						update.block,
						update.tree.root,
						update.tree.leaf_count,
					);
					continue;
				}
				if let Some(existing) = CreditTrees::<T>::get(update.block) {
					if existing != update.tree {
						// A block's credits are committed once and the root never changes
						// afterwards, so two roots for one block mean the chains disagree about
						// what that block awarded.
						log::error!(
							target: LOG_TARGET,
							"Conflicting credit tree for block {}: kept {:?}, ignored {:?}",
							update.block,
							existing.root,
							update.tree.root,
						);
						Self::deposit_event(Event::CreditTreeConflict { block: update.block });
					}
				} else {
					CreditTrees::<T>::insert(update.block, update.tree);
					stored = stored.saturating_add(1);
				}
			}

			Self::note_sequences(&batch);

			Self::deposit_event(Event::CreditTreesReceived { count, stored });

			Ok(())
		}

		/// Mints the NFT of one NFT claim credit the game chain awarded in `block`.
		///
		/// The credit is spent by the claim: its leaf is recorded, and a second claim of the same
		/// credit fails, whoever submits it.
		///
		/// ## Origin
		/// The signer of the claimant the credit was awarded to ([`Config::EnsureClaimant`]).
		///
		/// ## Parameters
		/// - `claimant`: Which of the signer's identities the credit was awarded to. A person
		///   claims as [`ClaimantKind::Person`], which resolves to the alias their account is bound
		///   to.
		/// - `block`: The People-chain block the credit was awarded in, which names the tree the
		///   proof is verified against.
		/// - `credit`: The credit being claimed. Hashed together with the origin's identity into
		///   the leaf, so a credit of somebody else's rehashes to a leaf that is in no tree.
		/// - `leaf_index`: The position of that leaf in the block's leaves, in award order.
		/// - `proof`: The sibling hashes that rehash the leaf up to the tree's root, bottom layer
		///   first, as the game chain's `nft_claim_credit_proofs` returns them.
		/// - `collection`: The Scarcity collection the NFT is minted into, which has to be
		///   registered through [`Pallet::set_collection_minter`]. Its [`ItemSelection`] decides
		///   the item.
		/// - `mint_to`: The Scarcity purse key the NFT is minted to. A purse key holds one NFT, so
		///   this has to be an empty one, and holders are meant to use a fresh key they control
		///   rather than an account that already holds something.
		#[pallet::call_index(1)]
		// Resolving a person claimant reads the signer's alias binding, which an account
		// claimant does not, so the kind the call names picks the weight.
		#[pallet::weight(
			match claimant {
				ClaimantKind::Account => T::WeightInfo::claim_account(proof.len() as u32),
				ClaimantKind::Person => T::WeightInfo::claim_person(proof.len() as u32),
			}
			.saturating_add(T::CollectionSelector::max_weight())
			.saturating_add(T::Nfts::mint_hook_weight(CLAIM_METADATA_PAIRS))
		)]
		pub fn claim(
			origin: OriginFor<T>,
			claimant: ClaimantKind,
			block: AwardBlock,
			credit: NftClaimCredit,
			leaf_index: u32,
			proof: BoundedVec<CreditProofNode, T::MaxProofNodes>,
			collection: CollectionId,
			mint_to: T::AccountId,
		) -> DispatchResultWithPostInfo {
			// Every failure carries `actual_weight` so the selector ceiling is refunded on the
			// error path too: a failed claim charges the claim's own weight plus what a failed
			// contract selection really consumed, not the whole reservation.
			let base = match claimant {
				ClaimantKind::Account => T::WeightInfo::claim_account(proof.len() as u32),
				ClaimantKind::Person => T::WeightInfo::claim_person(proof.len() as u32),
			};
			let claimant = T::EnsureClaimant::ensure_origin(origin, &claimant)
				.map_err(|e| e.with_weight(base))?;

			let tree = CreditTrees::<T>::get(block)
				.ok_or(Error::<T>::UnknownAwardBlock.with_weight(base))?;
			ensure!(
				leaf_index < tree.leaf_count,
				Error::<T>::LeafIndexOutOfBounds.with_weight(base)
			);

			let leaf = credit_leaf(&claimant, &credit);
			ensure!(
				!ClaimedCredits::<T>::contains_key(block, leaf),
				Error::<T>::AlreadyClaimed.with_weight(base)
			);

			// The root and the leaf count are the stored ones, never the claimant's: the count
			// decides how an odd layer was rehashed, so a caller-supplied one would select which
			// path is verified.
			ensure!(
				binary_merkle_tree::verify_proof::<BlakeTwo256, _, _>(
					&H256::from(tree.root),
					proof.iter().map(|node| H256::from(*node)),
					tree.leaf_count,
					leaf_index,
					&leaf,
				),
				Error::<T>::InvalidProof.with_weight(base)
			);

			// Spent before the selection so that a minter contract reentering with the same
			// credit fails `AlreadyClaimed`. A failure anywhere below unwinds the whole
			// dispatch, the entry included.
			ClaimedCredits::<T>::insert(block, leaf, ());

			let selection = Self::select_item(collection, credit).map_err(|error| {
				let error = error.into_claim_error::<T>();
				error.error.with_weight(base.saturating_add(error.weight_consumed))
			})?;
			let SelectedItem { item, weight_consumed: selection_weight, .. } = selection;
			let instance =
				T::Nfts::mint_without_deposit(collection, item, mint_to.clone(), Vec::new())
					.map_err(|e| e.with_weight(base.saturating_add(selection_weight)))?;

			// Counted after the selection: a contract may reenter with another credit of the
			// same block, and counting around its execution from a stale snapshot would drop
			// that claim's increment.
			let claimed = ClaimedCounts::<T>::mutate(block, |claimed| {
				*claimed = claimed.saturating_add(1);
				*claimed
			});

			Self::deposit_event(Event::CreditClaimed {
				block,
				leaf,
				collection,
				item,
				owner: mint_to,
				instance,
			});
			if claimed == tree.leaf_count {
				Self::deposit_event(Event::TreeFullyClaimed { block });
			}

			// The mint ran, so its runtime hooks did too. Only this path pays for them: every
			// failure above returns before the mint.
			Ok(Some(
				base.saturating_add(selection_weight)
					.saturating_add(T::Nfts::mint_hook_weight(CLAIM_METADATA_PAIRS)),
			)
			.into())
		}

		/// Registers `collection` for claims with `selection` deciding the minted item, or
		/// withdraws it with `None`.
		///
		/// Registration is the owner's opt-in to deposit-free supply growth: without it no claim
		/// can mint into the collection. Withdrawing stops further claims and spends nothing
		/// already claimed. Deleting the collection clears its registration through
		/// [`pallet_scarcity::OnCollectionDeleted`], so an unknown collection can be neither
		/// registered nor withdrawn. A contract selection is validated through
		/// [`CollectionSelector::validate`], so an address with no code fails here rather than on
		/// the first claim.
		///
		/// ## Origin
		/// The collection's Scarcity owner.
		///
		/// ## Parameters
		/// - `collection`: The Scarcity collection to register or withdraw.
		/// - `selection`: How claims pick the item to mint, or `None` to withdraw.
		#[pallet::call_index(2)]
		pub fn set_collection_minter(
			origin: OriginFor<T>,
			collection: CollectionId,
			selection: Option<ItemSelection>,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;
			let owner =
				T::Nfts::collection_owner(collection).ok_or(Error::<T>::UnknownCollection)?;
			ensure!(who == owner, Error::<T>::NotCollectionOwner);
			match selection {
				Some(selection) => {
					if let ItemSelection::Contract(contract) = selection {
						T::CollectionSelector::validate(contract)?;
					}
					CollectionMinters::<T>::insert(
						collection,
						CollectionMinter { owner, selection },
					);
				},
				None => CollectionMinters::<T>::remove(collection),
			}
			Self::deposit_event(Event::CollectionMinterSet { collection, selection });
			Ok(())
		}
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		#[cfg(feature = "std")]
		fn integrity_test() {
			assert!(
				T::MaxTreesPerMessage::get() > 0,
				"MaxTreesPerMessage must be greater than zero"
			);

			// A full batch arrives in an XCM `Transact` and is dispatched as one extrinsic, so a
			// worst case above the block's per-extrinsic limit can never execute: the message
			// fails and every tree in it is lost until it is replayed. The budget is the same
			// half of `Normal.max_extrinsic` the game pallet holds its sending side to, which
			// keeps the two ends of the delivery on one yardstick.
			OcwWeightBudget::from_normal_max::<T>().assert_fits(
				"receive_credit_trees",
				T::WeightInfo::receive_credit_trees(T::MaxTreesPerMessage::get()),
			);

			// A claim reserves the selector's ceiling on top of its own worst case whether the
			// collection uses a contract or not, so an unsubmittable worst case would make every
			// claim unsubmittable, not just contract-selected ones.
			OcwWeightBudget::from_normal_max::<T>().assert_fits(
				"claim",
				T::WeightInfo::claim_account(T::MaxProofNodes::get())
					.max(T::WeightInfo::claim_person(T::MaxProofNodes::get()))
					.saturating_add(T::CollectionSelector::max_weight())
					.saturating_add(T::Nfts::mint_hook_weight(CLAIM_METADATA_PAIRS)),
			);
		}

		#[cfg(feature = "try-runtime")]
		fn try_state(_n: BlockNumberFor<T>) -> Result<(), sp_runtime::TryRuntimeError> {
			Self::do_try_state()
		}
	}

	#[cfg(any(test, feature = "try-runtime"))]
	impl<T: Config> Pallet<T> {
		/// Check that the pallet's records agree with each other and with Scarcity: every
		/// claimed leaf belongs to a held tree, each block's claimed count matches its claimed
		/// leaves and never exceeds its tree's leaf count, and no registration outlives the
		/// collection it names.
		pub(crate) fn do_try_state() -> Result<(), sp_runtime::TryRuntimeError> {
			use alloc::collections::BTreeMap;
			use sp_runtime::TryRuntimeError;

			let mut counted = BTreeMap::<AwardBlock, u32>::new();
			for (block, _leaf, ()) in ClaimedCredits::<T>::iter() {
				if !CreditTrees::<T>::contains_key(block) {
					return Err(TryRuntimeError::Other("claimed credit has no tree"));
				}
				let count = counted.entry(block).or_default();
				*count = count
					.checked_add(1)
					.ok_or(TryRuntimeError::Other("claimed leaf count overflowed"))?;
			}

			let stored = ClaimedCounts::<T>::iter().collect::<BTreeMap<_, _>>();
			if stored != counted {
				return Err(TryRuntimeError::Other(
					"claimed counts do not match the claimed leaves",
				));
			}
			for (block, count) in &stored {
				let tree = CreditTrees::<T>::get(block)
					.ok_or(TryRuntimeError::Other("claimed count has no tree"))?;
				if *count > tree.leaf_count {
					return Err(TryRuntimeError::Other(
						"a block has more claims than its tree has leaves",
					));
				}
			}

			// Registration requires a live collection and deletion clears it through
			// `pallet_scarcity::OnCollectionDeleted`, so an entry naming a collection that no
			// longer exists means the runtime did not wire that hook to `ClearCollectionMinter`.
			// The registered owner is deliberately not compared against the current one: an
			// ownership handover leaves the registration stale on purpose, and claims reject it.
			for (collection, _) in CollectionMinters::<T>::iter() {
				if T::Nfts::collection_owner(collection).is_none() {
					return Err(TryRuntimeError::Other(
						"a collection minter registration outlived its collection",
					));
				}
			}
			Ok(())
		}
	}

	struct SelectedItem {
		item: ItemIndex,
		kind: crate::runtime_api::SelectionKind,
		weight_consumed: Weight,
	}

	enum ItemSelectionError {
		CollectionNotRegistered,
		UnknownCollection,
		CollectionOwnerChanged,
		NoItems,
		Contract(SelectionError),
	}

	impl ItemSelectionError {
		fn into_claim_error<T: Config>(self) -> SelectionError {
			let error = match self {
				Self::CollectionNotRegistered => Error::<T>::CollectionNotRegistered.into(),
				Self::UnknownCollection => Error::<T>::UnknownCollection.into(),
				Self::CollectionOwnerChanged => Error::<T>::CollectionOwnerChanged.into(),
				Self::NoItems => Error::<T>::NoItems.into(),
				Self::Contract(error) => return error,
			};
			SelectionError { error, weight_consumed: Weight::zero() }
		}

		fn into_preview_failure(self) -> crate::runtime_api::PreviewFailure {
			use crate::runtime_api::PreviewFailure;

			match self {
				Self::CollectionNotRegistered => PreviewFailure::CollectionNotRegistered,
				Self::UnknownCollection => PreviewFailure::UnknownCollection,
				Self::CollectionOwnerChanged => PreviewFailure::CollectionOwnerChanged,
				Self::NoItems => PreviewFailure::NoItems,
				Self::Contract(error) =>
					PreviewFailure::ContractSelectionFailed { error: error.error },
			}
		}
	}

	impl<T: Config> Pallet<T> {
		/// Previews the item the real claim selection path chooses for one credit and collection.
		/// Contract execution can change the current storage overlay, so runtime API callers must
		/// discard that overlay after the request.
		pub fn preview_mint(
			credit: NftClaimCredit,
			collection: CollectionId,
		) -> crate::runtime_api::PreviewOutcome {
			use crate::runtime_api::{PreviewFailure, PreviewOutcome};

			match Self::select_item(collection, credit) {
				Ok(selection) => {
					if !T::Nfts::item_exists(collection, selection.item) {
						return PreviewOutcome::Fails {
							reason: PreviewFailure::UnknownItem { item: selection.item },
						};
					}
					PreviewOutcome::Mints { item: selection.item, via: selection.kind }
				},
				Err(error) => PreviewOutcome::Fails { reason: error.into_preview_failure() },
			}
		}

		/// Previews a positionally aligned batch through the real claim selection path.
		/// Oversized batches fail explicitly before any selector runs.
		pub fn preview_mints(
			queries: Vec<crate::runtime_api::PreviewQuery>,
		) -> Result<Vec<crate::runtime_api::PreviewOutcome>, crate::runtime_api::BatchError> {
			if queries.len() > crate::runtime_api::MAX_PREVIEW_QUERIES as usize {
				return Err(crate::runtime_api::BatchError::TooLarge {
					max: crate::runtime_api::MAX_PREVIEW_QUERIES,
				});
			}
			Ok(queries
				.into_iter()
				.map(|query| Self::preview_mint(query.credit, query.collection))
				.collect::<Vec<_>>())
		}

		/// The item of `collection` that claiming `credit` mints, per the collection's
		/// registered [`ItemSelection`], with the weight the selection consumed.
		///
		/// A contract selection's failure is returned as its error: the contract is how the
		/// collection's owner gates minting, so no fallback overrides it. A failure carries the
		/// weight it consumed, so the claim charges what really ran.
		fn select_item(
			collection: CollectionId,
			credit: NftClaimCredit,
		) -> Result<SelectedItem, ItemSelectionError> {
			let registration = CollectionMinters::<T>::get(collection)
				.ok_or(ItemSelectionError::CollectionNotRegistered)?;
			let owner = T::Nfts::collection_owner(collection)
				.ok_or(ItemSelectionError::UnknownCollection)?;
			ensure!(owner == registration.owner, ItemSelectionError::CollectionOwnerChanged);
			match registration.selection {
				ItemSelection::Random => {
					let next_item = T::Nfts::next_item_index(collection)
						.ok_or(ItemSelectionError::UnknownCollection)?;
					ensure!(next_item > 0, ItemSelectionError::NoItems);
					let draw = u32::from_le_bytes(
						credit[..4].try_into().expect("a credit holds at least four bytes"),
					);
					Ok(SelectedItem {
						item: draw % next_item,
						kind: crate::runtime_api::SelectionKind::Random,
						weight_consumed: Weight::zero(),
					})
				},
				ItemSelection::Contract(contract) =>
					T::CollectionSelector::select(owner, contract, collection, credit)
						.map(|selection| SelectedItem {
							item: selection.item,
							kind: crate::runtime_api::SelectionKind::Contract(contract),
							weight_consumed: selection.weight_consumed,
						})
						.map_err(ItemSelectionError::Contract),
			}
		}

		/// Advances the expected sequence over the sequenced trees of `batch` and reports the
		/// ones that were skipped.
		///
		/// Only the highest sequence in the batch matters: trees arrive in ascending order, so
		/// anything below the expected sequence has already been accounted for, and one gap
		/// event covers a whole run of lost trees.
		fn note_sequences(batch: &CreditTreeBatch<T>) {
			let Some(highest) = batch.trees.iter().filter_map(|update| update.sequence).max()
			else {
				// A batch of resent trees only, which says nothing about the live stream.
				return;
			};

			let expected = NextExpectedSequence::<T>::get();
			if highest < expected {
				return;
			}

			let lowest =
				batch.trees.iter().filter_map(|update| update.sequence).min().unwrap_or(highest);
			if lowest > expected {
				Self::deposit_event(Event::CreditTreesMissing {
					from_sequence: expected,
					to_sequence: lowest.saturating_sub(1),
				});
			}

			NextExpectedSequence::<T>::put(highest.saturating_add(1));
		}
	}
}

impl<T: Config> Pallet<T> {
	/// The commitment held for `block`, which a claim for a credit awarded in that block is
	/// verified against.
	pub fn credit_tree(block: AwardBlock) -> Option<NftClaimCreditTree> {
		CreditTrees::<T>::get(block)
	}
}

/// Clears a collection's minter registration when Scarcity deletes the collection, so no
/// registration outlives the collection it names. The runtime wires this into
/// [`pallet_scarcity::Config::OnCollectionDeleted`].
pub struct ClearCollectionMinter<T>(core::marker::PhantomData<T>);

impl<T: Config> pallet_scarcity::OnCollectionDeleted for ClearCollectionMinter<T> {
	fn on_collection_deleted(collection: CollectionId) {
		CollectionMinters::<T>::remove(collection);
	}

	fn on_delete_weight() -> Weight {
		T::DbWeight::get().writes(1)
	}
}
