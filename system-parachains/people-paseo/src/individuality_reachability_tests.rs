// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Reachability tests for the individuality subsystem.
//!
//! These do not test pallet logic — the pallets have their own unit tests, and those tests pass
//! whether or not the pallet is callable on this chain. What they test is whether a wired pallet
//! can be *reached* at all through this runtime's own origin plumbing.
//!
//! The failure they exist to catch is silent. individuality upstream shipped a release in which
//! `indiv-pallet-people-airdrops` was wired, benchmarked and unit-tested, and completely
//! uncallable on chain: the runtime's `AccountContexts` registry did not list the pallet's
//! context, so `AsPerson` rejected every extrinsic with `InvalidTransaction::Call` during
//! validation. Nothing errored at build time, no event was emitted, and the pallet's own mock
//! configured an `EnsurePersonMock` that never consults `AccountContexts`. It was found on a live
//! network (see individuality `209586d`, "Register the people-airdrops alias context in
//! AccountContexts").
//!
//! Paseo carried the same class of defect independently: `AccountContexts` named
//! `indiv_pallet_score::SCORE_CONTEXT`, a constant v0.3.1 deleted in favour of a suffix-derived
//! `score_context()`.

use crate::{
	people::{AccountContexts, LitePeopleAccountContexts},
	Runtime,
};
use frame_support::traits::Contains;
use indiv_support::traits::Context;

/// Every context a pallet in this runtime gates its person origins on.
///
/// 🔴 **Adding a pallet whose `Config` binds `EnsurePersonalAliasInContext` means adding its
/// context here AND to `AccountContexts`.** Registering it in only one of the two is what the
/// test below exists to catch.
fn person_gated_contexts() -> alloc::vec::Vec<(&'static str, Context)> {
	alloc::vec![
		// `impl indiv_pallet_mob_rule::Config for Runtime { type EnsurePerson = … }`
		("mob-rule", indiv_pallet_mob_rule::MOB_CONTEXT),
		// `impl indiv_pallet_score::Config for Runtime { type EnsurePerson = … }`
		("score", indiv_pallet_score::Pallet::<Runtime>::score_context()),
		// `impl indiv_pallet_resources::Config for Runtime { type EnsurePerson = … }`
		("resources", indiv_pallet_resources::Pallet::<Runtime>::resources_context()),
	]
}

/// Every context a lite person may bind an account alias in.
///
/// 🔴 Same rule as above, against `LitePeopleAccountContexts`. `game` appears here because its
/// `Config::EnsureLiteAlias` is `EnsureLiteAliasInContext`, resolved in the *score* context.
fn lite_gated_contexts() -> alloc::vec::Vec<(&'static str, Context)> {
	alloc::vec![
		("people-lite auth", indiv_pallet_people_lite::Pallet::<Runtime>::auth_context()),
		("score (also game's EnsureLiteAlias)", indiv_pallet_score::Pallet::<Runtime>::score_context()),
	]
}

#[test]
fn every_person_gated_pallet_context_is_registered() {
	sp_io::TestExternalities::default().execute_with(|| {
		for (pallet, context) in person_gated_contexts() {
			assert!(
				AccountContexts::contains(&context),
				"`{pallet}` gates its person origin on a context that `AccountContexts` does not \
				 list. Every one of its person-origin extrinsics is unreachable on chain: \
				 `AsPerson` rejects them with `InvalidTransaction::Call` during validation, with \
				 no event and no log. Add the context to `AccountContexts::contains`.",
			);
		}
	});
}

#[test]
fn every_lite_gated_pallet_context_is_registered() {
	sp_io::TestExternalities::default().execute_with(|| {
		for (pallet, context) in lite_gated_contexts() {
			assert!(
				LitePeopleAccountContexts::contains(&context),
				"`{pallet}` resolves a lite alias in a context that `LitePeopleAccountContexts` \
				 does not list, so no lite person can bind an account alias for it.",
			);
		}
	});
}

#[test]
fn an_unregistered_context_is_rejected() {
	sp_io::TestExternalities::default().execute_with(|| {
		// A context no pallet in this runtime owns. If either registry accepted it, the tests
		// above would pass vacuously.
		let stranger: Context = *b"pop:polkadot.network/not-a-thing";
		assert!(!AccountContexts::contains(&stranger));
		assert!(!LitePeopleAccountContexts::contains(&stranger));
	});
}

#[test]
fn person_gated_contexts_are_distinct() {
	sp_io::TestExternalities::default().execute_with(|| {
		let contexts = person_gated_contexts();
		for (i, (a_name, a)) in contexts.iter().enumerate() {
			for (b_name, b) in contexts.iter().skip(i + 1) {
				assert_ne!(
					a, b,
					"`{a_name}` and `{b_name}` derive the same context, so an alias proved for \
					 one authorises the other. Contexts are the isolation boundary between \
					 products.",
				);
			}
		}
	});
}

/// The derived contexts must resolve against the **network-wide** suffix, the one Asset Hub also
/// derives from. Both runtimes default `NetworkSuffix` to
/// `system_parachains_constants::paseo::individuality::NETWORK_SUFFIX`, and the personhood
/// namespace is only shared while they agree: a suffix that differs between the two chains splits
/// every alias silently, on chain, with nothing to observe locally.
///
/// This also pins the contexts to a value, which is what a hard-coded byte string (the v0.3.0
/// shape that v0.3.1 deleted) would fail.
#[test]
fn derived_contexts_resolve_against_the_shared_network_suffix() {
	use indiv_support::context::{build_product_context, personhood};
	use system_parachains_constants::paseo::individuality::NETWORK_SUFFIX;

	sp_io::TestExternalities::default().execute_with(|| {
		let expect =
			|alloc| build_product_context(personhood::PRODUCT_NAME, NETWORK_SUFFIX, alloc);

		assert_eq!(
			indiv_pallet_score::Pallet::<Runtime>::score_context(),
			expect(personhood::SCORE),
		);
		assert_eq!(
			indiv_pallet_resources::Pallet::<Runtime>::resources_context(),
			expect(personhood::RESOURCES),
		);
		assert_eq!(
			indiv_pallet_people_lite::Pallet::<Runtime>::auth_context(),
			expect(personhood::PEOPLE_LITE_AUTH),
		);
	});
}

/// The suffix is on-chain state, not a compile-time constant: governance can rotate it through
/// `NetworkSuffix::set_network_suffix`. Every derived context must move with it, or a rotation
/// would strand the products that did not follow.
#[test]
fn rotating_the_network_suffix_moves_every_derived_context() {
	sp_io::TestExternalities::default().execute_with(|| {
		let before = (
			indiv_pallet_score::Pallet::<Runtime>::score_context(),
			indiv_pallet_resources::Pallet::<Runtime>::resources_context(),
			indiv_pallet_people_lite::Pallet::<Runtime>::auth_context(),
		);

		indiv_pallet_network_suffix::NetworkSuffix::<Runtime>::put(
			indiv_support::context::ProductContextNetworkSuffix::try_from(b"rotated".to_vec())
				.expect("fits the suffix bound"),
		);

		let after = (
			indiv_pallet_score::Pallet::<Runtime>::score_context(),
			indiv_pallet_resources::Pallet::<Runtime>::resources_context(),
			indiv_pallet_people_lite::Pallet::<Runtime>::auth_context(),
		);

		assert_ne!(before.0, after.0, "score context did not follow the suffix");
		assert_ne!(before.1, after.1, "resources context did not follow the suffix");
		assert_ne!(before.2, after.2, "people-lite auth context did not follow the suffix");
	});
}

/// Pins this runtime's transaction-extension pipeline, in execution order.
///
/// The origin modifiers run first and every extension that charges the transaction runs after
/// them, so an extension that installs an origin the payment extensions do not charge needs an
/// allowance in `pallet-origin-restriction` to bound it. An addition, a removal or a reorder
/// anywhere in this list is therefore a deliberate decision, not an implementation detail — and it
/// changes the transaction encoding, so it also requires a `transaction_version` bump.
///
/// 🔴 Slot 0 is `AuthorizeValueTransfer`, a Paseo-local deviation that upstream does not carry
/// (upstream has `()` there). It must survive every sync.
#[test]
fn the_transaction_extension_pipeline_is_the_expected_one() {
	use sp_runtime::traits::TransactionExtension;

	const PIPELINE: [&str; 23] = [
		"AuthorizeValueTransfer",
		"VerifyMultiSignature",
		"AsPerson",
		"AsProofOfInkParticipant",
		"ScoreAsParticipant",
		"GameAsInvited",
		"PeopleLiteAuth",
		"AsMember",
		"AsCoinage",
		"AsResources",
		"HonourAuth",
		"AuthorizeCall",
		"RestrictOrigins",
		"CheckNonZeroSender",
		"CheckSpecVersion",
		"CheckTxVersion",
		"CheckGenesis",
		"CheckMortality",
		"CheckNonce",
		"CheckWeight",
		"ChargeAssetTxPayment",
		"CheckMetadataHash",
		"StorageWeightReclaim",
	];

	let identifiers = <crate::TxExtension as TransactionExtension<crate::RuntimeCall>>::metadata()
		.into_iter()
		.map(|meta| meta.identifier)
		.collect::<alloc::vec::Vec<_>>();

	assert_eq!(identifiers, PIPELINE);
}
