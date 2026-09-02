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

//! NFT claims pallet benchmarking.

use super::*;
use crate::{
	pallet::{ClaimedCounts, ClaimedCredits, CollectionMinters, NextExpectedSequence},
	types::CreditTreeBatch,
	BenchmarkHelper,
};
use alloc::vec::Vec;
use frame_benchmarking::{v2::*, BenchmarkError};
use frame_support::{
	traits::{EnsureOrigin, EnsureOriginWithArg},
	BoundedVec,
};
use frame_system::RawOrigin;
use indiv_support::credit_trees::CreditTreeDelivery;

/// The `i`-th distinct credit a benchmarked tree commits to.
fn credit(i: u32) -> NftClaimCredit {
	let mut credit = [0u8; 32];
	credit[..4].copy_from_slice(&i.to_le_bytes());
	credit
}

/// A batch of `n` trees of the live stream, one per award block, as the game pallet sends it.
///
/// The sequence numbers start at one, so a receiver still expecting zero sees the batch as
/// ahead of the stream.
fn batch<T: Config>(n: u32) -> CreditTreeBatch<T> {
	let mut trees = BoundedVec::new();
	for i in 0..n {
		trees
			.try_push(CreditTreeDelivery {
				sequence: Some(i.saturating_add(1) as TreeSequence),
				block: i,
				tree: NftClaimCreditTree {
					game_index: 1,
					// Distinct per tree and never the zero root the pallet skips as invalid.
					root: CreditProofNode([i.saturating_add(1) as u8; 32]),
					leaf_count: 1,
					timestamp: 1_000,
				},
			})
			.expect("n is bounded by MaxTreesPerMessage; qed");
	}

	CreditTreeBatch::<T> { source_time: 1_000, trees }
}

/// Records the tree of an award block committing to `2^n` credits of `claimant` and returns what
/// a claim of one of them needs: the block, the credit, its leaf index and the sibling hashes.
///
/// The last leaf is picked, which is the deepest, so the proof carries all `n` hashes and every
/// one of them is rehashed before the root matches.
fn claimable_credit<T: Config>(
	claimant: &AccountOrPerson<T::AccountId>,
	n: u32,
) -> Result<
	(AwardBlock, NftClaimCredit, u32, BoundedVec<CreditProofNode, T::MaxProofNodes>),
	BenchmarkError,
> {
	let leaf_count = 1u32 << n;
	let credits = (0..leaf_count).map(credit).collect::<Vec<_>>();
	let leaves = credits
		.iter()
		.map(|credit| credit_leaf(claimant, credit))
		.collect::<Vec<NftClaimCreditLeaf>>();
	let leaf_index = leaf_count - 1;
	let proof = binary_merkle_tree::merkle_proof::<BlakeTwo256, _, _>(leaves, leaf_index);

	let block: AwardBlock = 1;
	CreditTrees::<T>::insert(
		block,
		NftClaimCreditTree { game_index: 1, root: proof.root.into(), leaf_count, timestamp: 1_000 },
	);

	let sibling_hashes = BoundedVec::try_from(
		proof.proof.into_iter().map(CreditProofNode::from).collect::<Vec<_>>(),
	)
	.map_err(|_| BenchmarkError::Stop("proof exceeds MaxProofNodes"))?;

	Ok((block, credits[leaf_index as usize], leaf_index, sibling_hashes))
}

#[benchmarks]
mod benches {
	use super::*;

	/// Worst case: every tree in the batch is new, so each one is written, and the batch is
	/// ahead of the expected sequence, so the gap is reported as well.
	#[benchmark]
	fn receive_credit_trees(
		n: Linear<1, { T::MaxTreesPerMessage::get() }>,
	) -> Result<(), BenchmarkError> {
		NextExpectedSequence::<T>::put(0);

		let batch = batch::<T>(n);
		let origin = T::EnsureGameChainOrigin::try_successful_origin()
			.map_err(|_| BenchmarkError::Stop("failed to construct game chain origin"))?;

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, batch);

		assert_eq!(CreditTrees::<T>::iter().count(), n as usize);
		assert_eq!(NextExpectedSequence::<T>::get(), n.saturating_add(1) as TreeSequence);

		Ok(())
	}

	/// Worst case: a tree of `2^n` leaves, so the proof carries `n` sibling hashes, every one of
	/// which is rehashed before the root matches. The claimed leaf is the last one and completes
	/// the tree, so the fully-claimed check passes and the event is emitted.
	///
	/// The claim is made under [`ClaimantKind::Account`], the kind whose origin check takes the
	/// signing account as it stands.
	///
	/// The collection is registered with [`ItemSelection::Random`], which is the branch this
	/// weight stands for: a contract selection adds the runtime selector's metered weight on
	/// top, reserved and refunded outside this function.
	#[benchmark]
	fn claim_account(n: Linear<0, { T::MaxProofNodes::get() }>) -> Result<(), BenchmarkError> {
		let kind = ClaimantKind::Account;
		let origin = T::EnsureClaimant::try_successful_origin(&kind)
			.map_err(|_| BenchmarkError::Stop("failed to construct claimant origin"))?;
		let claimant = T::EnsureClaimant::ensure_origin(origin.clone(), &kind)
			.map_err(|_| BenchmarkError::Stop("claimant origin does not resolve"))?;
		let mint_to: T::AccountId = account("purse", 0, 0);
		let owner: T::AccountId = account("collection-owner", 0, 0);
		let collection: CollectionId = 0;
		// One item, so the random draw resolves to it whatever the credit's bytes are.
		T::BenchmarkHelper::prepare_collection(&owner, collection, 0);
		CollectionMinters::<T>::insert(
			collection,
			CollectionMinter { owner, selection: ItemSelection::Random },
		);

		let (block, credit, leaf_index, sibling_hashes) = claimable_credit::<T>(&claimant, n)?;
		// Every other leaf is spent already, so the claim is the one that completes the tree.
		ClaimedCounts::<T>::insert(block, leaf_index);

		#[extrinsic_call]
		claim(
			origin as T::RuntimeOrigin,
			kind,
			block,
			credit,
			leaf_index,
			sibling_hashes,
			collection,
			mint_to,
		);

		assert_eq!(ClaimedCounts::<T>::get(block), leaf_index + 1);
		assert!(ClaimedCredits::<T>::contains_key(block, credit_leaf(&claimant, &credit)));

		Ok(())
	}

	/// Worst case: as `claim_account`, except that resolving [`ClaimantKind::Person`] has to look
	/// the signer's alias up rather than take the account as it stands.
	#[benchmark]
	fn claim_person(n: Linear<0, { T::MaxProofNodes::get() }>) -> Result<(), BenchmarkError> {
		let kind = ClaimantKind::Person;
		let origin = T::EnsureClaimant::try_successful_origin(&kind)
			.map_err(|_| BenchmarkError::Stop("failed to construct claimant origin"))?;
		let claimant = T::EnsureClaimant::ensure_origin(origin.clone(), &kind)
			.map_err(|_| BenchmarkError::Stop("claimant origin does not resolve"))?;
		let mint_to: T::AccountId = account("purse", 0, 0);
		let owner: T::AccountId = account("collection-owner", 0, 0);
		let collection: CollectionId = 0;
		// One item, so the random draw resolves to it whatever the credit's bytes are.
		T::BenchmarkHelper::prepare_collection(&owner, collection, 0);
		CollectionMinters::<T>::insert(
			collection,
			CollectionMinter { owner, selection: ItemSelection::Random },
		);

		let (block, credit, leaf_index, sibling_hashes) = claimable_credit::<T>(&claimant, n)?;
		// Every other leaf is spent already, so the claim is the one that completes the tree.
		ClaimedCounts::<T>::insert(block, leaf_index);

		#[extrinsic_call]
		claim(
			origin as T::RuntimeOrigin,
			kind,
			block,
			credit,
			leaf_index,
			sibling_hashes,
			collection,
			mint_to,
		);

		assert_eq!(ClaimedCounts::<T>::get(block), leaf_index + 1);
		assert!(ClaimedCredits::<T>::contains_key(block, credit_leaf(&claimant, &credit)));

		Ok(())
	}

	#[benchmark]
	fn set_collection_minter() -> Result<(), BenchmarkError> {
		let owner: T::AccountId = account("collection-owner", 0, 0);
		let collection: CollectionId = 0;
		T::BenchmarkHelper::prepare_collection(&owner, collection, 0);
		let contract = T::BenchmarkHelper::prepare_contract(&owner);

		#[extrinsic_call]
		_(RawOrigin::Signed(owner.clone()), collection, Some(ItemSelection::Contract(contract)));

		assert_eq!(
			CollectionMinters::<T>::get(collection),
			Some(CollectionMinter { owner, selection: ItemSelection::Contract(contract) })
		);

		Ok(())
	}

	impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
