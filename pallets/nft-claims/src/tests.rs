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

//! Tests for the nft-claims pallet.

use crate::{mock::*, ClaimantKind, Config, CreditTrees, Event, NextExpectedSequence, WeightInfo};
use frame_support::{assert_noop, assert_ok, dispatch::GetDispatchInfo, BoundedVec};
use indiv_support::credit_trees::{
	AwardBlock, CreditProofNode, CreditTreeDelivery, NftClaimCreditTree,
};
use sp_runtime::DispatchError;

#[test]
fn receive_credit_trees_stores_the_batch() {
	new_test_ext().execute_with(|| {
		assert_ok!(NftClaims::receive_credit_trees(
			game_chain_origin(),
			batch(vec![update(0, 10), update(1, 14)])
		));

		assert_eq!(CreditTrees::<Test>::get(10), Some(tree(10)));
		assert_eq!(CreditTrees::<Test>::get(14), Some(tree(14)));
		assert_eq!(NextExpectedSequence::<Test>::get(), 2);
		assert_eq!(nft_claims_events(), vec![Event::CreditTreesReceived { count: 2, stored: 2 }]);
	});
}

#[test]
fn a_stored_tree_is_readable_by_the_claim() {
	new_test_ext().execute_with(|| {
		assert_ok!(NftClaims::receive_credit_trees(
			game_chain_origin(),
			batch(vec![update(0, 10)])
		));

		assert_eq!(NftClaims::credit_tree(10), Some(tree(10)));
		assert_eq!(NftClaims::credit_tree(11), None);
	});
}

#[test]
fn receive_credit_trees_rejects_a_foreign_origin() {
	new_test_ext().execute_with(|| {
		assert_noop!(
			NftClaims::receive_credit_trees(
				RuntimeOrigin::signed(GAME_CHAIN + 1),
				batch(vec![update(0, 10)])
			),
			DispatchError::BadOrigin
		);
		assert_noop!(
			NftClaims::receive_credit_trees(RuntimeOrigin::root(), batch(vec![update(0, 10)])),
			DispatchError::BadOrigin
		);

		assert!(!CreditTrees::<Test>::contains_key(10));
	});
}

#[test]
fn a_replayed_tree_that_is_already_stored_changes_nothing() {
	new_test_ext().execute_with(|| {
		assert_ok!(NftClaims::receive_credit_trees(
			game_chain_origin(),
			batch(vec![update(0, 10)])
		));
		System::reset_events();

		assert_ok!(NftClaims::receive_credit_trees(game_chain_origin(), batch(vec![replay(10)])));

		assert_eq!(CreditTrees::<Test>::get(10), Some(tree(10)));
		assert_eq!(NextExpectedSequence::<Test>::get(), 1);
		assert_eq!(nft_claims_events(), vec![Event::CreditTreesReceived { count: 1, stored: 0 }]);
	});
}

#[test]
fn a_conflicting_root_keeps_the_stored_tree() {
	new_test_ext().execute_with(|| {
		assert_ok!(NftClaims::receive_credit_trees(
			game_chain_origin(),
			batch(vec![update(0, 10)])
		));
		System::reset_events();

		let conflicting = NftClaimCreditTree { root: CreditProofNode([0xff; 32]), ..tree(10) };
		assert_ok!(NftClaims::receive_credit_trees(
			game_chain_origin(),
			batch(vec![CreditTreeDelivery { sequence: None, block: 10, tree: conflicting }])
		));

		assert_eq!(CreditTrees::<Test>::get(10), Some(tree(10)));
		assert_eq!(
			nft_claims_events(),
			vec![
				Event::CreditTreeConflict { block: 10 },
				Event::CreditTreesReceived { count: 1, stored: 0 },
			]
		);
	});
}

#[test]
fn an_empty_or_zero_rooted_tree_is_skipped() {
	new_test_ext().execute_with(|| {
		let empty = NftClaimCreditTree { leaf_count: 0, ..tree(10) };
		let zero_rooted = NftClaimCreditTree { root: CreditProofNode([0u8; 32]), ..tree(11) };
		assert_ok!(NftClaims::receive_credit_trees(
			game_chain_origin(),
			batch(vec![
				CreditTreeDelivery { sequence: Some(0), block: 10, tree: empty },
				CreditTreeDelivery { sequence: Some(1), block: 11, tree: zero_rooted },
				update(2, 12),
			])
		));

		assert!(!CreditTrees::<Test>::contains_key(10));
		assert!(!CreditTrees::<Test>::contains_key(11));
		assert_eq!(CreditTrees::<Test>::get(12), Some(tree(12)));
		assert_eq!(nft_claims_events(), vec![Event::CreditTreesReceived { count: 3, stored: 1 }]);
	});
}

#[test]
fn a_sequence_gap_is_reported() {
	new_test_ext().execute_with(|| {
		assert_ok!(NftClaims::receive_credit_trees(
			game_chain_origin(),
			batch(vec![update(0, 10)])
		));
		System::reset_events();

		// Sequences 1 and 2 were lost on the way.
		assert_ok!(NftClaims::receive_credit_trees(
			game_chain_origin(),
			batch(vec![update(3, 20), update(4, 21)])
		));

		assert_eq!(NextExpectedSequence::<Test>::get(), 5);
		assert_eq!(
			nft_claims_events(),
			vec![
				Event::CreditTreesMissing { from_sequence: 1, to_sequence: 2 },
				Event::CreditTreesReceived { count: 2, stored: 2 },
			]
		);
	});
}

#[test]
fn a_contiguous_stream_reports_no_gap() {
	new_test_ext().execute_with(|| {
		assert_ok!(NftClaims::receive_credit_trees(
			game_chain_origin(),
			batch(vec![update(0, 10), update(1, 11)])
		));
		assert_ok!(NftClaims::receive_credit_trees(
			game_chain_origin(),
			batch(vec![update(2, 12)])
		));

		assert_eq!(NextExpectedSequence::<Test>::get(), 3);
		assert!(!nft_claims_events()
			.iter()
			.any(|event| matches!(event, Event::CreditTreesMissing { .. })));
	});
}

#[test]
fn a_replay_only_batch_leaves_the_expected_sequence_alone() {
	new_test_ext().execute_with(|| {
		assert_ok!(NftClaims::receive_credit_trees(
			game_chain_origin(),
			batch(vec![update(0, 10), update(1, 11)])
		));

		assert_ok!(NftClaims::receive_credit_trees(
			game_chain_origin(),
			batch(vec![replay(30), replay(31)])
		));

		assert_eq!(NextExpectedSequence::<Test>::get(), 2);
		assert_eq!(CreditTrees::<Test>::get(30), Some(tree(30)));
		assert!(!nft_claims_events()
			.iter()
			.any(|event| matches!(event, Event::CreditTreesMissing { .. })));
	});
}

#[test]
fn a_late_batch_below_the_expected_sequence_stores_but_reports_no_gap() {
	new_test_ext().execute_with(|| {
		assert_ok!(NftClaims::receive_credit_trees(
			game_chain_origin(),
			batch(vec![update(5, 20)])
		));
		System::reset_events();

		// A sequenced tree that arrives out of order must not rewind the expectation.
		assert_ok!(NftClaims::receive_credit_trees(
			game_chain_origin(),
			batch(vec![update(2, 15)])
		));

		assert_eq!(NextExpectedSequence::<Test>::get(), 6);
		assert_eq!(CreditTrees::<Test>::get(15), Some(tree(15)));
		assert_eq!(nft_claims_events(), vec![Event::CreditTreesReceived { count: 1, stored: 1 }]);
	});
}

mod claim {
	use super::*;
	use crate::{
		runtime_api::{
			BatchError, PreviewFailure, PreviewOutcome, PreviewQuery, SelectionKind,
			MAX_PREVIEW_QUERIES,
		},
		ClaimedCounts, ClaimedCredits, CollectionMinter, CollectionMinters, Error, ItemSelection,
	};
	use indiv_support::{credit_trees::credit_leaf, identity::AccountOrPerson};
	use pallet_scarcity::CollectionId;
	use sp_core::H160;

	/// Like [`assert_noop!`], but for `claim`: asserts the error kind and that no storage
	/// changed, ignoring the `actual_weight` the selector-reservation refund attaches to every
	/// failure. The refunded weight itself is asserted in
	/// [`a_failed_claim_refunds_the_selector_reservation_to_the_branch`].
	macro_rules! assert_claim_noop {
		($call:expr, $err:expr $(,)?) => {{
			let root = sp_io::storage::root(sp_runtime::StateVersion::V1);
			assert_eq!($call.map(|_| ()).map_err(|e| e.error), Err($err.into()));
			assert_eq!(
				root,
				sp_io::storage::root(sp_runtime::StateVersion::V1),
				"storage has been mutated"
			);
		}};
	}

	const ALICE: u64 = 1;
	const BOB: u64 = 2;
	const PURSE: u64 = 100;

	/// The block whose tree the tests claim against.
	const BLOCK: AwardBlock = 10;

	/// The collection the tests mint into.
	const COLLECTION: CollectionId = 3;
	/// The account owning [`COLLECTION`].
	const COLLECTION_OWNER: u64 = 50;
	/// The minter contract of [`COLLECTION`] when it is registered with a contract selection.
	const CONTRACT: H160 = H160::repeat_byte(0xcd);

	/// Make [`COLLECTION`] exist with two items and register it for claims with `selection`.
	///
	/// Two items keep the [`ItemSelection::Random`] draw meaningful: the item minted for a
	/// credit is its first four bytes modulo two.
	fn register_collection(selection: ItemSelection) {
		register_named_collection(COLLECTION, 2, selection);
	}

	/// Make a collection exist with `next_item_index` and register its claim selection.
	fn register_named_collection(
		collection: CollectionId,
		next_item_index: u32,
		selection: ItemSelection,
	) {
		add_collection(collection, COLLECTION_OWNER, next_item_index);
		CollectionMinters::<Test>::insert(
			collection,
			CollectionMinter { owner: COLLECTION_OWNER, selection },
		);
	}

	/// Three awards in one block: two of Alice's and one of the person's, in award order.
	fn awards() -> Vec<Award> {
		vec![
			(AccountOrPerson::Account(ALICE), [1u8; 32]),
			(AccountOrPerson::Person(PERSON_ALIAS), [2u8; 32]),
			(AccountOrPerson::Account(ALICE), [3u8; 32]),
		]
	}

	/// Store the tree of [`awards`] under [`BLOCK`], as a delivery from the game chain would,
	/// and register [`COLLECTION`] for claims with [`ItemSelection::Random`].
	fn store_tree(awards: &[Award]) {
		CreditTrees::<Test>::insert(BLOCK, tree_of(BLOCK, awards));
		register_collection(ItemSelection::Random);
	}

	#[test]
	fn an_account_claims_its_credit() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);

			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(ALICE),
				ClaimantKind::Account,
				BLOCK,
				[1u8; 32],
				0,
				proof_of(&awards, 0),
				COLLECTION,
				PURSE
			));

			let leaf = credit_leaf(&AccountOrPerson::Account(ALICE), &[1u8; 32]);
			assert!(ClaimedCredits::<Test>::contains_key(BLOCK, leaf));
			assert_eq!(ClaimedCounts::<Test>::get(BLOCK), 1);
			assert_eq!(MintedInstances::get(), vec![(3, 1, PURSE)]);
			assert_eq!(
				nft_claims_events(),
				vec![Event::CreditClaimed {
					block: BLOCK,
					leaf,
					collection: COLLECTION,
					item: 1,
					owner: PURSE,
					instance: 1
				}]
			);
		});
	}

	#[test]
	fn a_person_claims_the_credit_of_their_alias() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);

			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(PERSON),
				ClaimantKind::Person,
				BLOCK,
				[2u8; 32],
				1,
				proof_of(&awards, 1),
				COLLECTION,
				PURSE
			));

			let leaf = credit_leaf(&AccountOrPerson::<u64>::Person(PERSON_ALIAS), &[2u8; 32]);
			assert!(ClaimedCredits::<Test>::contains_key(BLOCK, leaf));
		});
	}

	/// The alias lookup a person claimant needs is charged only to that kind.
	#[test]
	fn the_declared_weight_follows_the_claimant_kind() {
		let weight_of = |claimant| {
			crate::Call::<Test>::claim {
				claimant,
				block: BLOCK,
				credit: [1u8; 32],
				leaf_index: 0,
				proof: BoundedVec::truncate_from(vec![CreditProofNode([7u8; 32]); 4]),
				collection: COLLECTION,
				mint_to: PURSE,
			}
			.get_dispatch_info()
			.call_weight
		};

		// Both kinds reserve the selector's ceiling and the mint hooks on top of their own weight.
		assert_eq!(
			weight_of(ClaimantKind::Account),
			<Test as Config>::WeightInfo::claim_account(4)
				.saturating_add(SELECTOR_MAX_WEIGHT)
				.saturating_add(MINT_HOOK_WEIGHT)
		);
		assert_eq!(
			weight_of(ClaimantKind::Person),
			<Test as Config>::WeightInfo::claim_person(4)
				.saturating_add(SELECTOR_MAX_WEIGHT)
				.saturating_add(MINT_HOOK_WEIGHT)
		);
		assert!(weight_of(ClaimantKind::Account).all_lt(weight_of(ClaimantKind::Person)));
	}

	#[test]
	fn every_leaf_of_a_tree_can_be_claimed_and_the_last_reports_the_tree_done() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);

			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(ALICE),
				ClaimantKind::Account,
				BLOCK,
				[1u8; 32],
				0,
				proof_of(&awards, 0),
				COLLECTION,
				PURSE
			));
			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(PERSON),
				ClaimantKind::Person,
				BLOCK,
				[2u8; 32],
				1,
				proof_of(&awards, 1),
				COLLECTION,
				PURSE + 1
			));
			System::reset_events();
			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(ALICE),
				ClaimantKind::Account,
				BLOCK,
				[3u8; 32],
				2,
				proof_of(&awards, 2),
				COLLECTION,
				PURSE + 2
			));

			assert_eq!(ClaimedCounts::<Test>::get(BLOCK), 3);
			assert!(nft_claims_events().contains(&Event::TreeFullyClaimed { block: BLOCK }));
		});
	}

	#[test]
	fn a_credit_is_claimed_once() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);
			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(ALICE),
				ClaimantKind::Account,
				BLOCK,
				[1u8; 32],
				0,
				proof_of(&awards, 0),
				COLLECTION,
				PURSE
			));

			assert_claim_noop!(
				NftClaims::claim(
					RuntimeOrigin::signed(ALICE),
					ClaimantKind::Account,
					BLOCK,
					[1u8; 32],
					0,
					proof_of(&awards, 0),
					COLLECTION,
					PURSE + 1
				),
				Error::<Test>::AlreadyClaimed
			);
			assert_eq!(ClaimedCounts::<Test>::get(BLOCK), 1);
		});
	}

	#[test]
	fn a_claim_of_another_claimants_credit_proves_nothing() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);

			// Bob presents Alice's credit, with the proof of her leaf: his own leaf is not in the
			// tree, so the proof cannot rehash to the root.
			assert_claim_noop!(
				NftClaims::claim(
					RuntimeOrigin::signed(BOB),
					ClaimantKind::Account,
					BLOCK,
					[1u8; 32],
					0,
					proof_of(&awards, 0),
					COLLECTION,
					PURSE
				),
				Error::<Test>::InvalidProof
			);
			// So does the person whose alias holds a different credit of the same tree.
			assert_claim_noop!(
				NftClaims::claim(
					RuntimeOrigin::signed(PERSON),
					ClaimantKind::Person,
					BLOCK,
					[1u8; 32],
					0,
					proof_of(&awards, 0),
					COLLECTION,
					PURSE
				),
				Error::<Test>::InvalidProof
			);
			assert!(MintedInstances::get().is_empty());
		});
	}

	#[test]
	fn a_claim_at_the_wrong_leaf_index_or_with_a_foreign_proof_fails() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);

			// Right credit, wrong position in the tree.
			assert_claim_noop!(
				NftClaims::claim(
					RuntimeOrigin::signed(ALICE),
					ClaimantKind::Account,
					BLOCK,
					[1u8; 32],
					2,
					proof_of(&awards, 0),
					COLLECTION,
					PURSE
				),
				Error::<Test>::InvalidProof
			);
			// Right position, but the proof belongs to another leaf.
			assert_claim_noop!(
				NftClaims::claim(
					RuntimeOrigin::signed(ALICE),
					ClaimantKind::Account,
					BLOCK,
					[1u8; 32],
					0,
					proof_of(&awards, 2),
					COLLECTION,
					PURSE
				),
				Error::<Test>::InvalidProof
			);
		});
	}

	#[test]
	fn a_claim_beyond_the_trees_leaves_fails() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);

			assert_claim_noop!(
				NftClaims::claim(
					RuntimeOrigin::signed(ALICE),
					ClaimantKind::Account,
					BLOCK,
					[1u8; 32],
					3,
					proof_of(&awards, 0),
					COLLECTION,
					PURSE
				),
				Error::<Test>::LeafIndexOutOfBounds
			);
		});
	}

	#[test]
	fn a_claim_against_a_tree_that_has_not_arrived_fails() {
		new_test_ext().execute_with(|| {
			let awards = awards();

			assert_claim_noop!(
				NftClaims::claim(
					RuntimeOrigin::signed(ALICE),
					ClaimantKind::Account,
					BLOCK,
					[1u8; 32],
					0,
					proof_of(&awards, 0),
					COLLECTION,
					PURSE
				),
				Error::<Test>::UnknownAwardBlock
			);
		});
	}

	#[test]
	fn a_claim_against_another_blocks_root_fails() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			// A different set of awards is committed under the block being claimed against, so
			// the proof is well-formed but rehashes to a root this block does not hold.
			store_tree(&[(AccountOrPerson::Account(BOB), [7u8; 32])]);

			assert_claim_noop!(
				NftClaims::claim(
					RuntimeOrigin::signed(ALICE),
					ClaimantKind::Account,
					BLOCK,
					[1u8; 32],
					0,
					proof_of(&awards, 0),
					COLLECTION,
					PURSE
				),
				Error::<Test>::InvalidProof
			);
		});
	}

	#[test]
	fn a_claim_rejects_an_origin_that_is_neither_an_account_nor_a_person() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);

			assert_claim_noop!(
				NftClaims::claim(
					RuntimeOrigin::root(),
					ClaimantKind::Account,
					BLOCK,
					[1u8; 32],
					0,
					proof_of(&awards, 0),
					COLLECTION,
					PURSE
				),
				DispatchError::BadOrigin
			);
		});
	}

	#[test]
	fn a_person_claiming_under_their_account_proves_nothing() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);

			// The credit is the person's, but the account the signer resolves to under
			// `ClaimantKind::Account` hashes into a leaf that is in no tree.
			assert_claim_noop!(
				NftClaims::claim(
					RuntimeOrigin::signed(PERSON),
					ClaimantKind::Account,
					BLOCK,
					[2u8; 32],
					1,
					proof_of(&awards, 1),
					COLLECTION,
					PURSE
				),
				Error::<Test>::InvalidProof
			);
			assert!(MintedInstances::get().is_empty());
		});
	}

	#[test]
	fn a_signer_with_no_alias_binding_cannot_claim_as_a_person() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);

			assert_claim_noop!(
				NftClaims::claim(
					RuntimeOrigin::signed(ALICE),
					ClaimantKind::Person,
					BLOCK,
					[2u8; 32],
					1,
					proof_of(&awards, 1),
					COLLECTION,
					PURSE
				),
				DispatchError::BadOrigin
			);
		});
	}

	#[test]
	fn a_failing_mint_leaves_the_credit_unspent() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);
			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(ALICE),
				ClaimantKind::Account,
				BLOCK,
				[1u8; 32],
				0,
				proof_of(&awards, 0),
				COLLECTION,
				PURSE
			));

			// The purse key already holds an NFT, so the mint fails and the second credit stays
			// claimable at another key.
			assert_claim_noop!(
				NftClaims::claim(
					RuntimeOrigin::signed(ALICE),
					ClaimantKind::Account,
					BLOCK,
					[3u8; 32],
					2,
					proof_of(&awards, 2),
					COLLECTION,
					PURSE
				),
				DispatchError::Other("AddressOccupied")
			);
			assert_eq!(ClaimedCounts::<Test>::get(BLOCK), 1);
			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(ALICE),
				ClaimantKind::Account,
				BLOCK,
				[3u8; 32],
				2,
				proof_of(&awards, 2),
				COLLECTION,
				PURSE + 1
			));
		});
	}

	#[test]
	fn a_claim_into_an_unregistered_collection_fails() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);
			CollectionMinters::<Test>::remove(COLLECTION);

			assert_claim_noop!(
				NftClaims::claim(
					RuntimeOrigin::signed(ALICE),
					ClaimantKind::Account,
					BLOCK,
					[1u8; 32],
					0,
					proof_of(&awards, 0),
					COLLECTION,
					PURSE
				),
				Error::<Test>::CollectionNotRegistered
			);
			assert!(MintedInstances::get().is_empty());
			assert_eq!(ClaimedCounts::<Test>::get(BLOCK), 0);
		});
	}

	#[test]
	fn a_new_collection_owner_must_register_before_claims_resume() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);
			let new_owner = COLLECTION_OWNER + 1;
			add_collection(COLLECTION, new_owner, 2);

			assert_claim_noop!(
				NftClaims::claim(
					RuntimeOrigin::signed(ALICE),
					ClaimantKind::Account,
					BLOCK,
					[1u8; 32],
					0,
					proof_of(&awards, 0),
					COLLECTION,
					PURSE
				),
				Error::<Test>::CollectionOwnerChanged
			);
			assert!(MintedInstances::get().is_empty());
			assert_eq!(ClaimedCounts::<Test>::get(BLOCK), 0);

			assert_ok!(NftClaims::set_collection_minter(
				RuntimeOrigin::signed(new_owner),
				COLLECTION,
				Some(ItemSelection::Random)
			));
			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(ALICE),
				ClaimantKind::Account,
				BLOCK,
				[1u8; 32],
				0,
				proof_of(&awards, 0),
				COLLECTION,
				PURSE
			));
			assert_eq!(MintedInstances::get(), vec![(COLLECTION, 1, PURSE)]);
		});
	}

	#[test]
	fn a_random_selection_draws_the_item_from_the_credit() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);

			// The draw is the credit's first four bytes modulo the two items: `[1u8; 32]` is
			// odd, `[2u8; 32]` even.
			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(ALICE),
				ClaimantKind::Account,
				BLOCK,
				[1u8; 32],
				0,
				proof_of(&awards, 0),
				COLLECTION,
				PURSE
			));
			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(PERSON),
				ClaimantKind::Person,
				BLOCK,
				[2u8; 32],
				1,
				proof_of(&awards, 1),
				COLLECTION,
				PURSE + 1
			));

			assert_eq!(
				MintedInstances::get(),
				vec![(COLLECTION, 1, PURSE), (COLLECTION, 0, PURSE + 1)]
			);
			assert!(SelectorCalls::get().is_empty());
		});
	}

	#[test]
	fn preview_and_claim_share_random_pure_and_stateful_selection() {
		new_test_ext().execute_with(|| {
			const PURE_COLLECTION: CollectionId = 4;
			const STATEFUL_COLLECTION: CollectionId = 5;
			const PURE_CONTRACT: H160 = H160::repeat_byte(0xaa);
			const STATEFUL_CONTRACT: H160 = H160::repeat_byte(0xbb);

			let awards = awards();
			CreditTrees::<Test>::insert(BLOCK, tree_of(BLOCK, &awards));
			register_named_collection(COLLECTION, 4, ItemSelection::Random);
			register_named_collection(PURE_COLLECTION, 8, ItemSelection::Contract(PURE_CONTRACT));
			register_named_collection(
				STATEFUL_COLLECTION,
				8,
				ItemSelection::Contract(STATEFUL_CONTRACT),
			);
			SelectorItem::set(&5);
			StatefulSelectorContract::set(&Some(STATEFUL_CONTRACT));
			StatefulSelectorItem::set(&2);

			let preview = frame_support::storage::with_transaction(|| {
				frame_support::storage::TransactionOutcome::Rollback(
					Result::<_, DispatchError>::Ok(NftClaims::preview_mints(vec![
						PreviewQuery { credit: [1u8; 32], collection: COLLECTION },
						PreviewQuery { credit: [2u8; 32], collection: PURE_COLLECTION },
						PreviewQuery { credit: [3u8; 32], collection: STATEFUL_COLLECTION },
					])),
				)
			})
			.expect("the preview transaction starts")
			.expect("three queries fit the preview cap");
			assert_eq!(
				preview,
				vec![
					PreviewOutcome::Mints { item: 1, via: SelectionKind::Random },
					PreviewOutcome::Mints { item: 5, via: SelectionKind::Contract(PURE_CONTRACT) },
					PreviewOutcome::Mints {
						item: 2,
						via: SelectionKind::Contract(STATEFUL_CONTRACT),
					},
				]
			);
			assert_eq!(StatefulSelectorItem::get(), 2);

			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(ALICE),
				ClaimantKind::Account,
				BLOCK,
				[1u8; 32],
				0,
				proof_of(&awards, 0),
				COLLECTION,
				PURSE
			));
			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(PERSON),
				ClaimantKind::Person,
				BLOCK,
				[2u8; 32],
				1,
				proof_of(&awards, 1),
				PURE_COLLECTION,
				PURSE + 1
			));
			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(ALICE),
				ClaimantKind::Account,
				BLOCK,
				[3u8; 32],
				2,
				proof_of(&awards, 2),
				STATEFUL_COLLECTION,
				PURSE + 2
			));

			let previewed_items = preview
				.into_iter()
				.map(|outcome| match outcome {
					PreviewOutcome::Mints { item, .. } => item,
					PreviewOutcome::Fails { reason } => panic!("preview failed: {reason:?}"),
				})
				.collect::<Vec<_>>();
			let minted_items =
				MintedInstances::get().into_iter().map(|(_, item, _)| item).collect::<Vec<_>>();
			assert_eq!(previewed_items, minted_items);
		});
	}

	#[test]
	fn preview_batch_compounds_repeated_stateful_selections() {
		new_test_ext().execute_with(|| {
			const STATEFUL_COLLECTION: CollectionId = 5;
			const STATEFUL_CONTRACT: H160 = H160::repeat_byte(0xbb);

			register_named_collection(
				STATEFUL_COLLECTION,
				8,
				ItemSelection::Contract(STATEFUL_CONTRACT),
			);
			StatefulSelectorContract::set(&Some(STATEFUL_CONTRACT));
			StatefulSelectorItem::set(&2);

			let preview = frame_support::storage::with_transaction(|| {
				frame_support::storage::TransactionOutcome::Rollback(
					Result::<_, DispatchError>::Ok(NftClaims::preview_mints(vec![
						PreviewQuery { credit: [1u8; 32], collection: STATEFUL_COLLECTION },
						PreviewQuery { credit: [2u8; 32], collection: STATEFUL_COLLECTION },
					])),
				)
			})
			.expect("the preview transaction starts")
			.expect("two queries fit the preview cap");

			// The second query sees the first query's contract state, exactly as claiming in
			// that order would; the shared overlay is then discarded whole.
			assert_eq!(
				preview,
				vec![
					PreviewOutcome::Mints {
						item: 2,
						via: SelectionKind::Contract(STATEFUL_CONTRACT),
					},
					PreviewOutcome::Mints {
						item: 3,
						via: SelectionKind::Contract(STATEFUL_CONTRACT),
					},
				]
			);
			assert_eq!(StatefulSelectorItem::get(), 2);
		});
	}

	#[test]
	fn preview_reports_selection_failures_without_failing_the_batch() {
		new_test_ext().execute_with(|| {
			const UNREGISTERED: CollectionId = 10;
			const OWNER_CHANGED: CollectionId = 11;
			const DELETED_ITEM: CollectionId = 12;
			const CONTRACT_FAILURE: CollectionId = 13;
			const NO_ITEMS: CollectionId = 14;
			const DELETED_COLLECTION: CollectionId = 15;

			add_collection(UNREGISTERED, COLLECTION_OWNER, 2);
			register_named_collection(OWNER_CHANGED, 2, ItemSelection::Random);
			add_collection(OWNER_CHANGED, COLLECTION_OWNER + 1, 2);
			register_named_collection(DELETED_ITEM, 2, ItemSelection::Random);
			MissingItems::set(&vec![(DELETED_ITEM, 1)]);
			register_named_collection(CONTRACT_FAILURE, 2, ItemSelection::Contract(CONTRACT));
			register_named_collection(NO_ITEMS, 0, ItemSelection::Random);
			register_named_collection(DELETED_COLLECTION, 2, ItemSelection::Random);
			let mut collections = MockCollections::get();
			collections.retain(|(collection, _, _)| *collection != DELETED_COLLECTION);
			MockCollections::set(&collections);
			SelectorFails::set(&true);

			assert_eq!(
				NftClaims::preview_mints(vec![
					PreviewQuery { credit: [1u8; 32], collection: UNREGISTERED },
					PreviewQuery { credit: [1u8; 32], collection: OWNER_CHANGED },
					PreviewQuery { credit: [1u8; 32], collection: DELETED_ITEM },
					PreviewQuery { credit: [1u8; 32], collection: CONTRACT_FAILURE },
					PreviewQuery { credit: [1u8; 32], collection: NO_ITEMS },
					PreviewQuery { credit: [1u8; 32], collection: DELETED_COLLECTION },
				]),
				Ok(vec![
					PreviewOutcome::Fails { reason: PreviewFailure::CollectionNotRegistered },
					PreviewOutcome::Fails { reason: PreviewFailure::CollectionOwnerChanged },
					PreviewOutcome::Fails { reason: PreviewFailure::UnknownItem { item: 1 } },
					PreviewOutcome::Fails {
						reason: PreviewFailure::ContractSelectionFailed {
							error: DispatchError::Other("SelectorFailed"),
						},
					},
					PreviewOutcome::Fails { reason: PreviewFailure::NoItems },
					PreviewOutcome::Fails { reason: PreviewFailure::UnknownCollection },
				])
			);
		});
	}

	#[test]
	fn preview_rejects_an_oversized_batch_before_selection() {
		new_test_ext().execute_with(|| {
			assert_eq!(
				NftClaims::preview_mints(vec![
					PreviewQuery {
						credit: [0u8; 32],
						collection: COLLECTION
					};
					MAX_PREVIEW_QUERIES as usize + 1
				]),
				Err(BatchError::TooLarge { max: MAX_PREVIEW_QUERIES })
			);
			assert!(SelectorCalls::get().is_empty());
		});
	}

	#[test]
	fn a_random_selection_with_no_items_fails() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);
			add_collection(COLLECTION, COLLECTION_OWNER, 0);

			assert_claim_noop!(
				NftClaims::claim(
					RuntimeOrigin::signed(ALICE),
					ClaimantKind::Account,
					BLOCK,
					[1u8; 32],
					0,
					proof_of(&awards, 0),
					COLLECTION,
					PURSE
				),
				Error::<Test>::NoItems
			);
		});
	}

	#[test]
	fn a_random_selection_against_a_deleted_collection_fails() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);
			// The collection is gone from the backend while its registration lingers.
			MockCollections::set(&Vec::new());

			assert_claim_noop!(
				NftClaims::claim(
					RuntimeOrigin::signed(ALICE),
					ClaimantKind::Account,
					BLOCK,
					[1u8; 32],
					0,
					proof_of(&awards, 0),
					COLLECTION,
					PURSE
				),
				Error::<Test>::UnknownCollection
			);
		});
	}

	#[test]
	fn a_contract_selection_asks_the_registered_contract() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);
			register_collection(ItemSelection::Contract(CONTRACT));
			add_collection(COLLECTION, COLLECTION_OWNER, 10);
			SelectorItem::set(&9);

			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(ALICE),
				ClaimantKind::Account,
				BLOCK,
				[1u8; 32],
				0,
				proof_of(&awards, 0),
				COLLECTION,
				PURSE
			));

			// The contract is called with the credit as the only entropy and the claim mints
			// exactly the item it picked.
			assert_eq!(
				SelectorCalls::get(),
				vec![(COLLECTION_OWNER, CONTRACT, COLLECTION, [1u8; 32])]
			);
			assert_eq!(MintedInstances::get(), vec![(COLLECTION, 9, PURSE)]);
		});
	}

	#[test]
	fn a_minter_reentering_with_the_same_credit_fails_already_claimed() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);
			register_collection(ItemSelection::Contract(CONTRACT));

			// The selector claims the same credit mid-selection, as a reentrant minter
			// contract would. The credit is spent before the selector runs, so the nested
			// claim fails and the outer claim still mints exactly once.
			SelectorReentry::set(&Some(ReentrantClaim {
				claimant: ALICE,
				kind: ClaimantKind::Account,
				block: BLOCK,
				credit: [1u8; 32],
				leaf_index: 0,
				proof: proof_of(&awards, 0).into_inner(),
				collection: COLLECTION,
				mint_to: PURSE + 1,
			}));

			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(ALICE),
				ClaimantKind::Account,
				BLOCK,
				[1u8; 32],
				0,
				proof_of(&awards, 0),
				COLLECTION,
				PURSE
			));

			assert_eq!(ReentryResult::get(), Some(Err(Error::<Test>::AlreadyClaimed.into())));
			assert_eq!(ClaimedCounts::<Test>::get(BLOCK), 1);
			assert_eq!(MintedInstances::get().len(), 1);
		});
	}

	#[test]
	fn a_minter_reentering_with_another_credit_loses_neither_claims_count() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);
			register_collection(ItemSelection::Contract(CONTRACT));

			// The selector claims a different credit of the same block mid-selection. The
			// claimed count is read after the selector returns, so the nested claim's
			// increment is not overwritten from a stale snapshot.
			SelectorReentry::set(&Some(ReentrantClaim {
				claimant: ALICE,
				kind: ClaimantKind::Account,
				block: BLOCK,
				credit: [3u8; 32],
				leaf_index: 2,
				proof: proof_of(&awards, 2).into_inner(),
				collection: COLLECTION,
				mint_to: PURSE + 1,
			}));

			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(ALICE),
				ClaimantKind::Account,
				BLOCK,
				[1u8; 32],
				0,
				proof_of(&awards, 0),
				COLLECTION,
				PURSE
			));

			assert_eq!(ReentryResult::get(), Some(Ok(())));
			assert_eq!(ClaimedCounts::<Test>::get(BLOCK), 2);
			assert_eq!(MintedInstances::get().len(), 2);
			let outer = credit_leaf(&AccountOrPerson::Account(ALICE), &[1u8; 32]);
			let nested = credit_leaf(&AccountOrPerson::Account(ALICE), &[3u8; 32]);
			assert!(ClaimedCredits::<Test>::contains_key(BLOCK, outer));
			assert!(ClaimedCredits::<Test>::contains_key(BLOCK, nested));
		});
	}

	#[test]
	fn a_failing_contract_leaves_the_credit_unspent() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);
			register_collection(ItemSelection::Contract(CONTRACT));
			SelectorFails::set(&true);

			assert_claim_noop!(
				NftClaims::claim(
					RuntimeOrigin::signed(ALICE),
					ClaimantKind::Account,
					BLOCK,
					[1u8; 32],
					0,
					proof_of(&awards, 0),
					COLLECTION,
					PURSE
				),
				DispatchError::Other("SelectorFailed")
			);

			let leaf = credit_leaf(&AccountOrPerson::Account(ALICE), &[1u8; 32]);
			assert!(!ClaimedCredits::<Test>::contains_key(BLOCK, leaf));
			assert_eq!(ClaimedCounts::<Test>::get(BLOCK), 0);
			assert!(MintedInstances::get().is_empty());

			// The gate is the contract's to lift: once it stops failing, the credit claims.
			SelectorFails::set(&false);
			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(ALICE),
				ClaimantKind::Account,
				BLOCK,
				[1u8; 32],
				0,
				proof_of(&awards, 0),
				COLLECTION,
				PURSE
			));
		});
	}

	#[test]
	fn try_state_holds_after_claims_and_catches_corrupted_accounting() {
		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);
			assert_ok!(NftClaims::do_try_state());

			assert_ok!(NftClaims::claim(
				RuntimeOrigin::signed(ALICE),
				ClaimantKind::Account,
				BLOCK,
				[1u8; 32],
				0,
				proof_of(&awards, 0),
				COLLECTION,
				PURSE
			));
			assert_ok!(NftClaims::do_try_state());

			// A count that disagrees with the claimed leaves is the corruption the check is for.
			ClaimedCounts::<Test>::insert(BLOCK, 2);
			assert!(NftClaims::do_try_state().is_err());
		});
	}

	#[test]
	fn try_state_catches_a_registration_that_outlived_its_collection() {
		new_test_ext().execute_with(|| {
			register_collection(ItemSelection::Random);
			assert_ok!(NftClaims::do_try_state());

			// What a runtime that left `OnCollectionDeleted` unwired would leave behind: a
			// registration naming a collection that no longer answers for an owner.
			CollectionMinters::<Test>::insert(
				COLLECTION + 1,
				CollectionMinter { owner: COLLECTION_OWNER, selection: ItemSelection::Random },
			);
			assert!(NftClaims::do_try_state().is_err());
		});
	}

	#[test]
	fn a_claim_refunds_the_selector_reservation_to_the_branch_taken() {
		use crate::weights::WeightInfo;
		use frame_support::dispatch::GetDispatchInfo;

		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);

			let proof = proof_of(&awards, 0);
			let charged = crate::Call::<Test>::claim {
				claimant: ClaimantKind::Account,
				block: BLOCK,
				credit: [1u8; 32],
				leaf_index: 0,
				proof: proof.clone(),
				collection: COLLECTION,
				mint_to: PURSE,
			}
			.get_dispatch_info()
			.call_weight;
			// A successful claim mints, so it also pays for the mint's runtime hooks. Only the
			// selector reservation is refundable.
			let account_minted = <Test as Config>::WeightInfo::claim_account(proof.len() as u32)
				.saturating_add(MINT_HOOK_WEIGHT);
			let person_minted = <Test as Config>::WeightInfo::claim_person(proof.len() as u32)
				.saturating_add(MINT_HOOK_WEIGHT);

			// The random branch consumes no selector weight, so the whole reservation comes
			// back.
			let post = NftClaims::claim(
				RuntimeOrigin::signed(ALICE),
				ClaimantKind::Account,
				BLOCK,
				[1u8; 32],
				0,
				proof.clone(),
				COLLECTION,
				PURSE,
			)
			.expect("random-branch claim succeeds");
			assert_eq!(post.actual_weight, Some(account_minted));
			assert!(account_minted.all_lt(charged));

			// The contract branch pays what the selector really consumed, still below the
			// reservation.
			register_collection(ItemSelection::Contract(CONTRACT));
			let post = NftClaims::claim(
				RuntimeOrigin::signed(PERSON),
				ClaimantKind::Person,
				BLOCK,
				[2u8; 32],
				1,
				proof_of(&awards, 1),
				COLLECTION,
				PURSE + 1,
			)
			.expect("contract-branch claim succeeds");
			assert_eq!(
				post.actual_weight,
				Some(person_minted.saturating_add(SELECTOR_CONSUMED_WEIGHT))
			);
			assert!(post.actual_weight.expect("set above").all_lt(charged));
		});
	}

	#[test]
	fn a_failed_claim_refunds_the_selector_reservation_to_the_branch() {
		use crate::weights::WeightInfo;
		use frame_support::dispatch::GetDispatchInfo;

		new_test_ext().execute_with(|| {
			let awards = awards();
			store_tree(&awards);

			let proof = proof_of(&awards, 0);
			let charged = crate::Call::<Test>::claim {
				claimant: ClaimantKind::Account,
				block: BLOCK,
				credit: [1u8; 32],
				leaf_index: 0,
				proof: proof.clone(),
				collection: COLLECTION,
				mint_to: PURSE,
			}
			.get_dispatch_info()
			.call_weight;
			let account_base = <Test as Config>::WeightInfo::claim_account(proof.len() as u32);
			let person_base = <Test as Config>::WeightInfo::claim_person(proof.len() as u32);

			// A failure before any selection refunds the whole reservation: a bad proof never
			// reaches a minter, so a failing claim cannot occupy the selector's block space.
			let err = NftClaims::claim(
				RuntimeOrigin::signed(ALICE),
				ClaimantKind::Account,
				BLOCK,
				[1u8; 32],
				2,
				proof.clone(),
				COLLECTION,
				PURSE,
			)
			.expect_err("the wrong leaf index fails the proof");
			assert_eq!(err.error, Error::<Test>::InvalidProof.into());
			assert_eq!(err.post_info.actual_weight, Some(account_base));
			assert!(account_base.all_lt(charged));

			// A failing contract selection pays what the contract really burned before failing,
			// above the pre-selection floor and still below the reservation.
			register_collection(ItemSelection::Contract(CONTRACT));
			SelectorFails::set(&true);
			let err = NftClaims::claim(
				RuntimeOrigin::signed(PERSON),
				ClaimantKind::Person,
				BLOCK,
				[2u8; 32],
				1,
				proof_of(&awards, 1),
				COLLECTION,
				PURSE + 1,
			)
			.expect_err("the contract selection fails");
			assert_eq!(err.error, DispatchError::Other("SelectorFailed"));
			let actual = err.post_info.actual_weight.expect("a failure refunds to a weight");
			assert_eq!(actual, person_base.saturating_add(SELECTOR_FAILED_WEIGHT));
			assert!(person_base.all_lt(actual));
			assert!(actual.all_lt(charged));
		});
	}
}

mod set_collection_minter {
	use super::*;
	use crate::{CollectionMinter, CollectionMinters, Error, Event, ItemSelection};
	use sp_core::H160;

	const OWNER: u64 = 50;
	const COLLECTION: u32 = 3;
	const CONTRACT: H160 = H160::repeat_byte(0xcd);

	#[test]
	fn the_owner_registers_and_withdraws_a_collection() {
		new_test_ext().execute_with(|| {
			add_collection(COLLECTION, OWNER, 2);

			let selection = ItemSelection::Contract(CONTRACT);
			assert_ok!(NftClaims::set_collection_minter(
				RuntimeOrigin::signed(OWNER),
				COLLECTION,
				Some(selection)
			));
			assert_eq!(
				CollectionMinters::<Test>::get(COLLECTION),
				Some(CollectionMinter { owner: OWNER, selection })
			);

			assert_ok!(NftClaims::set_collection_minter(
				RuntimeOrigin::signed(OWNER),
				COLLECTION,
				None
			));
			assert_eq!(CollectionMinters::<Test>::get(COLLECTION), None);

			assert_eq!(
				nft_claims_events(),
				vec![
					Event::CollectionMinterSet {
						collection: COLLECTION,
						selection: Some(selection)
					},
					Event::CollectionMinterSet { collection: COLLECTION, selection: None },
				]
			);
		});
	}

	#[test]
	fn only_the_owner_registers() {
		new_test_ext().execute_with(|| {
			add_collection(COLLECTION, OWNER, 2);

			assert_noop!(
				NftClaims::set_collection_minter(
					RuntimeOrigin::signed(OWNER + 1),
					COLLECTION,
					Some(ItemSelection::Random)
				),
				Error::<Test>::NotCollectionOwner
			);
			assert_eq!(CollectionMinters::<Test>::get(COLLECTION), None);
		});
	}

	#[test]
	fn an_unknown_collection_cannot_be_registered() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				NftClaims::set_collection_minter(
					RuntimeOrigin::signed(OWNER),
					COLLECTION,
					Some(ItemSelection::Random)
				),
				Error::<Test>::UnknownCollection
			);
			assert_noop!(
				NftClaims::set_collection_minter(RuntimeOrigin::signed(OWNER), COLLECTION, None),
				Error::<Test>::UnknownCollection
			);
		});
	}

	#[test]
	fn deleting_a_collection_clears_its_registration() {
		new_test_ext().execute_with(|| {
			add_collection(COLLECTION, OWNER, 2);
			assert_ok!(NftClaims::set_collection_minter(
				RuntimeOrigin::signed(OWNER),
				COLLECTION,
				Some(ItemSelection::Random)
			));

			// Scarcity calls the deletion hook when the collection is deleted, so no
			// registration outlives the collection it names and none can be stranded.
			<crate::ClearCollectionMinter<Test> as pallet_scarcity::OnCollectionDeleted>::on_collection_deleted(
				COLLECTION,
			);
			assert_eq!(CollectionMinters::<Test>::get(COLLECTION), None);
		});
	}
	#[test]
	fn an_address_without_code_cannot_be_registered_as_a_minter() {
		new_test_ext().execute_with(|| {
			add_collection(COLLECTION, OWNER, 2);
			ContractValid::set(&false);

			assert_noop!(
				NftClaims::set_collection_minter(
					RuntimeOrigin::signed(OWNER),
					COLLECTION,
					Some(ItemSelection::Contract(CONTRACT))
				),
				sp_runtime::DispatchError::Other("NotAContract")
			);
			assert_eq!(CollectionMinters::<Test>::get(COLLECTION), None);

			// Only a contract selection is validated: the random one names no contract.
			assert_ok!(NftClaims::set_collection_minter(
				RuntimeOrigin::signed(OWNER),
				COLLECTION,
				Some(ItemSelection::Random)
			));
		});
	}
}
