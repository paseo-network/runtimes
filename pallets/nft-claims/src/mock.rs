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

//! Mock runtime for the nft-claims pallet tests.

use crate::{
	self as pallet_nft_claims, ClaimantKind, CollectionSelector, Event, Selection, SelectionError,
};
use frame_support::{
	derive_impl, parameter_types,
	traits::{EnsureOrigin, EnsureOriginWithArg},
	weights::Weight,
	BoundedVec,
};
use indiv_support::{
	credit_trees::{
		credit_leaf, AwardBlock, CreditProofNode, CreditTreeDelivery, NftClaimCredit,
		NftClaimCreditLeaf, NftClaimCreditTree, TreeSequence,
	},
	identity::AccountOrPerson,
	traits::Alias,
};
use pallet_scarcity::{CollectionId, InspectCollection, InstanceId, ItemIndex, MintWithoutDeposit};
use sp_core::H160;
use sp_runtime::{traits::BlakeTwo256, BuildStorage, DispatchError};

pub type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
	pub enum Test
	{
		System: frame_system,
		NftClaims: pallet_nft_claims,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type Block = Block;
	type AccountData = ();
}

/// The account the mock accepts as the game chain's XCM origin.
pub const GAME_CHAIN: u64 = 7;

pub struct MockEnsureGameChainOrigin;
impl EnsureOrigin<RuntimeOrigin> for MockEnsureGameChainOrigin {
	type Success = ();

	fn try_origin(o: RuntimeOrigin) -> Result<Self::Success, RuntimeOrigin> {
		match o.clone().into() {
			Ok(frame_system::RawOrigin::Signed(who)) if who == GAME_CHAIN => Ok(()),
			_ => Err(o),
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
		Ok(RuntimeOrigin::signed(GAME_CHAIN))
	}
}

/// The only account the mock has an alias binding for, standing in for the `AccountToAlias` entry
/// alias-accounts holds on the claim chain.
pub const PERSON: u64 = 42;
/// The alias [`PERSON`] is bound to.
pub const PERSON_ALIAS: Alias = [9u8; 32];

pub struct MockEnsureClaimant;
impl EnsureOriginWithArg<RuntimeOrigin, ClaimantKind> for MockEnsureClaimant {
	type Success = AccountOrPerson<u64>;

	fn try_origin(o: RuntimeOrigin, kind: &ClaimantKind) -> Result<Self::Success, RuntimeOrigin> {
		match (o.clone().into(), kind) {
			(Ok(frame_system::RawOrigin::Signed(who)), ClaimantKind::Account) =>
				Ok(AccountOrPerson::Account(who)),
			(Ok(frame_system::RawOrigin::Signed(PERSON)), ClaimantKind::Person) =>
				Ok(AccountOrPerson::Person(PERSON_ALIAS)),
			// A signer with no alias binding cannot claim as a person.
			_ => Err(o),
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn try_successful_origin(kind: &ClaimantKind) -> Result<RuntimeOrigin, ()> {
		Ok(match kind {
			ClaimantKind::Account => RuntimeOrigin::signed(1),
			ClaimantKind::Person => RuntimeOrigin::signed(PERSON),
		})
	}
}

/// Weights that tell the two claimant kinds apart, so a test of the weight the `claim` call
/// declares fails if the call charges the wrong branch.
pub struct MockWeightInfo;
impl pallet_nft_claims::WeightInfo for MockWeightInfo {
	fn receive_credit_trees(n: u32) -> Weight {
		Weight::from_parts(1_000 + 10 * n as u64, 0)
	}

	fn claim_account(n: u32) -> Weight {
		Weight::from_parts(2_000 + 10 * n as u64, 100)
	}

	fn claim_person(n: u32) -> Weight {
		Weight::from_parts(5_000 + 10 * n as u64, 200)
	}

	fn set_collection_minter() -> Weight {
		Weight::from_parts(3_000, 50)
	}
}

parameter_types! {
	pub const MaxTreesPerMessage: u32 = 4;
	pub const MaxProofNodes: u32 = 16;
	/// The instances the mock minter has handed out, as `(collection, item, owner)` in mint order.
	pub storage MintedInstances: Vec<(CollectionId, ItemIndex, u64)> = Vec::new();
	/// The collections the mock backend holds, as `(collection, owner, next_item_index)`.
	pub storage MockCollections: Vec<(CollectionId, u64, ItemIndex)> = Vec::new();
	/// Allocated item definitions removed from the mock backend.
	pub storage MissingItems: Vec<(CollectionId, ItemIndex)> = Vec::new();
	/// The selections the mock selector was asked for, in call order.
	pub storage SelectorCalls: Vec<(u64, H160, CollectionId, NftClaimCredit)> = Vec::new();
	/// Whether the mock selector fails, standing in for a trapped or reverting contract.
	pub storage SelectorFails: bool = false;
	/// A claim the selector submits from inside the selection, standing in for a minter
	/// contract calling back into the runtime. Taken before dispatching so it runs once.
	pub storage SelectorReentry: Option<ReentrantClaim> = None;
	/// What the reentrant claim returned, for the test to assert on.
	pub storage ReentryResult: Option<Result<(), DispatchError>> = None;
	/// The item the mock selector picks.
	pub storage SelectorItem: ItemIndex = 0;
	/// The contract whose selected item advances after each call, when set.
	pub storage StatefulSelectorContract: Option<H160> = None;
	/// The next item picked by [`StatefulSelectorContract`].
	pub storage StatefulSelectorItem: ItemIndex = 0;
	/// Whether the mock selector accepts a registered contract address as deployed code.
	pub storage ContractValid: bool = true;
}

/// Make `collection` exist in the mock backend, owned by `owner` with items `0..next_item_index`.
pub fn add_collection(collection: CollectionId, owner: u64, next_item_index: ItemIndex) {
	let mut collections = MockCollections::get();
	collections.retain(|(existing, _, _)| *existing != collection);
	collections.push((collection, owner, next_item_index));
	MockCollections::set(&collections);
}

/// Stands in for Scarcity: allocates instance identifiers in mint order and enforces the one
/// NFT per purse key rule, which is the failure a claim has to propagate.
pub struct MockNfts;
impl MintWithoutDeposit<u64> for MockNfts {
	type MetadataKey = Vec<u8>;
	type MetadataValue = Vec<u8>;

	fn mint_without_deposit(
		collection: CollectionId,
		item: ItemIndex,
		to: u64,
		_metadata: Vec<(Self::MetadataKey, Self::MetadataValue)>,
	) -> Result<InstanceId, DispatchError> {
		if !<Self as InspectCollection<u64>>::item_exists(collection, item) {
			return Err(DispatchError::Other("UnknownItem"));
		}
		let mut minted = MintedInstances::get();
		if minted.iter().any(|(_, _, owner)| *owner == to) {
			return Err(DispatchError::Other("AddressOccupied"));
		}
		minted.push((collection, item, to));
		let instance = minted.len() as InstanceId;
		MintedInstances::set(&minted);
		Ok(instance)
	}

	fn mint_hook_weight(pairs: u32) -> Weight {
		MINT_HOOK_WEIGHT.saturating_add(Weight::from_parts(pairs as u64, 0))
	}
}

/// Weight the mock reports for the mint's runtime hooks, distinct from zero so that the
/// claim's charge and refund assertions cannot pass tautologically.
pub const MINT_HOOK_WEIGHT: Weight = Weight::from_parts(9_000, 300);

impl InspectCollection<u64> for MockNfts {
	fn collection_owner(collection: CollectionId) -> Option<u64> {
		MockCollections::get()
			.iter()
			.find(|(existing, _, _)| *existing == collection)
			.map(|(_, owner, _)| *owner)
	}

	fn next_item_index(collection: CollectionId) -> Option<ItemIndex> {
		MockCollections::get()
			.iter()
			.find(|(existing, _, _)| *existing == collection)
			.map(|(_, _, next_item_index)| *next_item_index)
	}

	fn item_exists(collection: CollectionId, item: ItemIndex) -> bool {
		Self::next_item_index(collection).is_some_and(|next_item| item < next_item) &&
			!MissingItems::get().contains(&(collection, item))
	}
}

/// The worst case the mock selector charges, distinct from what it consumes so that refund
/// assertions cannot pass tautologically.
pub const SELECTOR_MAX_WEIGHT: Weight = Weight::from_parts(1_000_000, 5_000);
/// The weight the mock selector reports as really consumed.
pub const SELECTOR_CONSUMED_WEIGHT: Weight = Weight::from_parts(400_000, 1_000);
/// The weight the mock selector reports for a failed call, distinct from the successful
/// consumption and from zero so error-refund assertions cannot pass tautologically.
pub const SELECTOR_FAILED_WEIGHT: Weight = Weight::from_parts(250_000, 700);

/// The arguments of a claim the mock selector submits mid-selection through [`SelectorReentry`].
#[derive(Clone, PartialEq, Eq, Debug, codec::Encode, codec::Decode)]
pub struct ReentrantClaim {
	pub claimant: u64,
	pub kind: ClaimantKind,
	pub block: AwardBlock,
	pub credit: NftClaimCredit,
	pub leaf_index: u32,
	pub proof: Vec<CreditProofNode>,
	pub collection: CollectionId,
	pub mint_to: u64,
}

/// Stands in for the runtime's contract adapter: records its calls and picks [`SelectorItem`],
/// or fails when [`SelectorFails`] says so. When [`SelectorReentry`] holds a claim, the
/// selector dispatches it before returning, as a reentrant minter contract would.
pub struct MockSelector;
impl CollectionSelector<u64> for MockSelector {
	fn max_weight() -> Weight {
		SELECTOR_MAX_WEIGHT
	}

	fn validate(_contract: H160) -> Result<(), DispatchError> {
		if !ContractValid::get() {
			return Err(DispatchError::Other("NotAContract"));
		}
		Ok(())
	}

	fn select(
		owner: u64,
		contract: H160,
		collection: CollectionId,
		entropy: NftClaimCredit,
	) -> Result<Selection, SelectionError> {
		let mut calls = SelectorCalls::get();
		calls.push((owner, contract, collection, entropy));
		SelectorCalls::set(&calls);
		if let Some(reentry) = SelectorReentry::get() {
			SelectorReentry::set(&None);
			let proof =
				BoundedVec::try_from(reentry.proof).expect("the reentrant proof fits the bound");
			let result = NftClaims::claim(
				RuntimeOrigin::signed(reentry.claimant),
				reentry.kind,
				reentry.block,
				reentry.credit,
				reentry.leaf_index,
				proof,
				reentry.collection,
				reentry.mint_to,
			);
			ReentryResult::set(&Some(result.map(|_| ()).map_err(|e| e.error)));
		}
		if SelectorFails::get() {
			return Err(SelectionError {
				error: DispatchError::Other("SelectorFailed"),
				weight_consumed: SELECTOR_FAILED_WEIGHT,
			});
		}
		let item = if StatefulSelectorContract::get() == Some(contract) {
			let item = StatefulSelectorItem::get();
			StatefulSelectorItem::set(&item.saturating_add(1));
			item
		} else {
			SelectorItem::get()
		};
		Ok(Selection { item, weight_consumed: SELECTOR_CONSUMED_WEIGHT })
	}
}

#[cfg(feature = "runtime-benchmarks")]
pub struct MockBenchmarkHelper;
#[cfg(feature = "runtime-benchmarks")]
impl pallet_nft_claims::BenchmarkHelper<u64> for MockBenchmarkHelper {
	fn prepare_collection(owner: &u64, collection: CollectionId, item: ItemIndex) {
		add_collection(collection, *owner, item.saturating_add(1));
	}

	fn prepare_contract(_owner: &u64) -> H160 {
		H160::repeat_byte(1)
	}
}

impl pallet_nft_claims::Config for Test {
	type WeightInfo = MockWeightInfo;
	type EnsureGameChainOrigin = MockEnsureGameChainOrigin;
	type MaxTreesPerMessage = MaxTreesPerMessage;
	type EnsureClaimant = MockEnsureClaimant;
	type Nfts = MockNfts;
	type CollectionSelector = MockSelector;
	type MaxProofNodes = MaxProofNodes;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = MockBenchmarkHelper;
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	let storage = frame_system::GenesisConfig::<Test>::default().build_storage().unwrap();
	let mut ext: sp_io::TestExternalities = storage.into();
	ext.execute_with(|| System::set_block_number(1));
	ext
}

pub fn game_chain_origin() -> RuntimeOrigin {
	RuntimeOrigin::signed(GAME_CHAIN)
}

/// A credit tree whose fields are derived from `block`, so trees of different blocks differ.
pub fn tree(block: AwardBlock) -> NftClaimCreditTree {
	NftClaimCreditTree {
		game_index: 7,
		root: CreditProofNode([block as u8; 32]),
		leaf_count: 3,
		timestamp: 1_000 + block,
	}
}

/// One update of the live stream: a tree with the sequence number it was delivered under.
pub fn update(sequence: TreeSequence, block: AwardBlock) -> CreditTreeDelivery {
	CreditTreeDelivery { sequence: Some(sequence), block, tree: tree(block) }
}

/// One resent update, which carries no sequence number.
pub fn replay(block: AwardBlock) -> CreditTreeDelivery {
	CreditTreeDelivery { sequence: None, block, tree: tree(block) }
}

/// A batch of `updates`, as the game pallet assembles it.
pub fn batch(updates: Vec<CreditTreeDelivery>) -> crate::CreditTreeBatch<Test> {
	crate::CreditTreeBatch::<Test> {
		source_time: 1_000,
		trees: updates.try_into().expect("batch fits MaxTreesPerMessage"),
	}
}

/// One award as its block recorded it: who holds the credit and the credit itself.
pub type Award = (AccountOrPerson<u64>, NftClaimCredit);

/// The leaves of `awards`, in award order, exactly as the game chain hashes them.
pub fn leaves(awards: &[Award]) -> Vec<NftClaimCreditLeaf> {
	awards.iter().map(|(claimant, credit)| credit_leaf(claimant, credit)).collect()
}

/// The tree `awards` commit to, with the root the game chain would have recorded for `block`.
pub fn tree_of(block: AwardBlock, awards: &[Award]) -> NftClaimCreditTree {
	NftClaimCreditTree {
		game_index: 7,
		root: binary_merkle_tree::merkle_root::<BlakeTwo256, _>(leaves(awards)).into(),
		leaf_count: awards.len() as u32,
		timestamp: 1_000 + block,
	}
}

/// The inclusion proof of the award at `leaf_index` in `awards`.
pub fn proof_of(awards: &[Award], leaf_index: u32) -> BoundedVec<CreditProofNode, MaxProofNodes> {
	let proof = binary_merkle_tree::merkle_proof::<BlakeTwo256, _, _>(leaves(awards), leaf_index);
	proof
		.proof
		.into_iter()
		.map(CreditProofNode::from)
		.collect::<Vec<_>>()
		.try_into()
		.expect("a proof of a mock tree fits MaxProofNodes")
}

/// The events the pallet emitted so far.
pub fn nft_claims_events() -> Vec<Event<Test>> {
	System::events()
		.into_iter()
		.filter_map(|record| match record.event {
			RuntimeEvent::NftClaims(event) => Some(event),
			_ => None,
		})
		.collect()
}
