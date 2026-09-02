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

//! A mock runtime for the credits, which is the game's own with this pallet on top: the credits sit
//! above the game pallet, so the pair is what a test runs.
//!
//! Everything but the pallet list and what only the credits need comes from [`runtime`], the file
//! the game pallet's mock is built from as well. See its module documentation for why it is
//! included rather than depended on.

#[path = "../../game/src/mock_runtime.rs"]
mod runtime;

pub use runtime::*;

use crate::WeightInfo as CreditsWeightInfo;
use codec::{Decode, Encode};
use cumulus_primitives_core::ParaId;
use frame_support::parameter_types;
// `mock_runtime.rs` names the game pallet's crate through its parent module, and here that is the
// crate this pallet sits on top of.
pub use indiv_pallet_game;
use sp_runtime::Weight;
use std::cell::RefCell;

/// The credits are this pallet's, so the runtime `mock_runtime.rs` builds awards them here: the
/// game's tests see a report award a credit, and these tests see what becomes of it.
type MockNftClaimCredits = NftCredits;

// Configure a mock runtime to test the pallet.
frame_support::construct_runtime!(
	pub enum Test
	{
		System: frame_system,
		ChunksManager: indiv_pallet_chunks_manager,
		Members: indiv_pallet_members,
		Game: indiv_pallet_game,
		NftCredits: crate,
		Score: indiv_pallet_score,
		Balances: pallet_balances,
		Assets: pallet_assets,
		AssetsHolder: pallet_assets_holder,
		Airdrop: indiv_pallet_airdrop,
		People: indiv_pallet_people,
		PeopleLite: indiv_pallet_people_lite,
		Deposit: deposit,
	}
);

impl crate::Config for Test {
	type WeightInfo = MockWeightInfo;
	type MaxCreditsPerBlock = MaxCreditsPerBlock;
	type XcmRouter = MockXcmRouter;
	type NftClaimsParaId = NftClaimsParaId;
	type NftClaimsPalletIndex = NftClaimsPalletIndex;
	type ChannelInfo = MockChannelInfo;
	type MaxQueuedCreditTrees = MaxQueuedCreditTrees;
	type MaxCreditTreesPerMessage = MaxCreditTreesPerMessage;
	type ReplayCooldownSeconds = ReplayCooldownSeconds;
	type NftClaimsRemoteWeight = NftClaimsRemoteWeight;
	type MaxRetainedAwardBlocks = MaxRetainedAwardBlocks;
	type MaxCreditBlocksPerClaimant = MaxCreditBlocksPerClaimant;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = MockCreditsBenchmarkHelper;
}

impl CreditsWeightInfo for MockWeightInfo {
	fn build_credit_tree(_n: u32) -> Weight {
		Weight::zero()
	}
	fn build_credit_tree_empty() -> Weight {
		Weight::zero()
	}
	/// Non-zero and scaling with `n` in both dimensions, so that the refund
	/// `send_credit_trees` reports is strictly below its worst case.
	fn send_credit_trees(n: u32) -> Weight {
		Weight::from_parts(100 + 10 * n as u64, 10 + n as u64)
	}
	fn replay_credit_trees(n: u32) -> Weight {
		Weight::from_parts(200 + 10 * n as u64, 20 + n as u64)
	}
	fn authorize_send_credit_trees() -> Weight {
		Weight::from_parts(50, 0)
	}
}

parameter_types! {
	/// Generous by default: tests submit every player's report within one block, which a real
	/// block's weight limit would never allow. Tests covering a full leaf buffer lower it.
	pub storage MaxCreditsPerBlock: u32 = 20_000;
	pub storage MaxRetainedAwardBlocks: u32 = 16;
	/// Tests covering the index dropping its oldest block lower it.
	pub storage MaxCreditBlocksPerClaimant: u32 = 16;
	pub const NftClaimsParaId: ParaId = ParaId::new(1000);
	pub const NftClaimsPalletIndex: u8 = 42;
	/// Small, so that a test can fill the delivery queue in a few blocks.
	pub storage MaxQueuedCreditTrees: u32 = 8;
	pub storage MaxCreditTreesPerMessage: u32 = 4;
	pub const ReplayCooldownSeconds: u64 = 60;
	pub const NftClaimsRemoteWeight: Weight = Weight::from_parts(1_000, 0);
}

/// Captures the XCM messages the pallet sends to the NFT claims chain and can be made to fail
/// so that a test can drive the retry path.
///
/// A message larger than the channel takes is rejected, as the XCMP queue rejects it on chain,
/// counting what the routing stack adds after the pallet has handed the message over: the unique
/// topic and the page format byte the fragment is measured with.
pub struct MockXcmRouter;

impl xcm::latest::SendXcm for MockXcmRouter {
	type Ticket = (xcm::latest::Location, Vec<u8>);

	fn validate(
		destination: &mut Option<xcm::latest::Location>,
		message: &mut Option<xcm::latest::Xcm<()>>,
	) -> xcm::latest::SendResult<Self::Ticket> {
		let destination = destination.take().unwrap_or(xcm::latest::Location::here());
		let mut message = message.take().unwrap_or_default();
		message.0.push(xcm::latest::Instruction::SetTopic([0u8; 32]));

		let fragment = xcm::VersionedXcm::<()>::from(message.clone()).encode().len() +
			cumulus_primitives_core::XcmpMessageFormat::ConcatenatedVersionedXcm.encoded_size();
		if fragment > MOCK_CLAIMS_MAX_MESSAGE_SIZE.with_borrow(|size| *size) as usize {
			return Err(xcm::latest::SendError::ExceedsMaxMessageSize);
		}

		Ok(((destination, message.encode()), xcm::latest::Assets::new()))
	}

	fn deliver(ticket: Self::Ticket) -> Result<xcm::latest::XcmHash, xcm::latest::SendError> {
		if XCM_SEND_SHOULD_FAIL.with_borrow(|fail| *fail) {
			return Err(xcm::latest::SendError::Transport("mock failure"));
		}
		SENT_XCMS.with_borrow_mut(|sent| sent.push(ticket));
		Ok([0u8; 32])
	}
}

/// Stands in for `ParachainSystem`'s view of the HRMP channel to the NFT claims chain.
pub struct MockChannelInfo;

impl cumulus_primitives_core::GetChannelInfo for MockChannelInfo {
	fn get_channel_status(_id: ParaId) -> cumulus_primitives_core::ChannelStatus {
		cumulus_primitives_core::ChannelStatus::Ready(0, 0)
	}

	fn get_channel_info(_id: ParaId) -> Option<cumulus_primitives_core::ChannelInfo> {
		if !MOCK_HAS_CLAIMS_CHANNEL.with_borrow(|open| *open) {
			return None;
		}
		Some(cumulus_primitives_core::ChannelInfo {
			max_capacity: 1000,
			max_total_size: 102_400 * 1000,
			max_message_size: MOCK_CLAIMS_MAX_MESSAGE_SIZE.with_borrow(|size| *size),
			msg_count: 0,
			total_size: 0,
		})
	}
}

/// The XCM messages sent to the NFT claims chain so far.
pub fn sent_credit_tree_xcms() -> Vec<(xcm::latest::Location, Vec<u8>)> {
	SENT_XCMS.with_borrow(|sent| sent.clone())
}

/// The batch of credit trees carried by the last XCM message, with the pallet index and call
/// index it was addressed to checked along the way.
pub fn last_sent_credit_tree_batch() -> crate::CreditTreeBatch<Test> {
	use xcm::latest::{Instruction, Xcm};

	let (_, encoded) = sent_credit_tree_xcms().pop().expect("an XCM was sent");
	let message: Xcm<()> = Xcm::decode(&mut &encoded[..]).expect("XCM decodes");
	let call = message
		.0
		.into_iter()
		.find_map(|instruction| match instruction {
			Instruction::Transact { call, .. } => Some(call.into_encoded()),
			_ => None,
		})
		.expect("the XCM carries a Transact");

	assert_eq!(call[0], NftClaimsPalletIndex::get(), "addressed to the nft-claims pallet");
	assert_eq!(call[1], 0, "the call index of `receive_credit_trees`");
	crate::CreditTreeBatch::<Test>::decode(&mut &call[2..]).expect("the batch decodes")
}

/// Makes the next XCM send fail, as a closed or congested channel would.
pub fn fail_credit_tree_xcms(fail: bool) {
	XCM_SEND_SHOULD_FAIL.with_borrow_mut(|flag| *flag = fail);
}

/// Closes the HRMP channel to the NFT claims chain.
pub fn close_claims_channel() {
	MOCK_HAS_CLAIMS_CHANNEL.with_borrow_mut(|open| *open = false);
}

/// Shrinks the HRMP channel to the NFT claims chain to `size` bytes per message.
pub fn set_claims_max_message_size(size: u32) {
	MOCK_CLAIMS_MAX_MESSAGE_SIZE.with_borrow_mut(|current| *current = size);
}

thread_local! {
	/// See [`MockXcmRouter`].
	static XCM_SEND_SHOULD_FAIL: RefCell<bool> = const { RefCell::new(false) };
	/// See [`MockXcmRouter`].
	static SENT_XCMS: RefCell<Vec<(xcm::latest::Location, Vec<u8>)>> =
		const { RefCell::new(Vec::new()) };
	/// See [`MockChannelInfo`].
	static MOCK_HAS_CLAIMS_CHANNEL: RefCell<bool> = const { RefCell::new(true) };
	/// See [`MockChannelInfo`].
	static MOCK_CLAIMS_MAX_MESSAGE_SIZE: RefCell<u32> = const { RefCell::new(100_000) };
}

/// The shared runtime's externalities, with what only the credits keep in thread-local state reset
/// on top. Shadows [`runtime::new_test_ext`], which knows nothing of the credits.
pub fn new_test_ext() -> sp_io::TestExternalities {
	SENT_XCMS.with_borrow_mut(|sent| sent.clear());
	XCM_SEND_SHOULD_FAIL.with_borrow_mut(|fail| *fail = false);
	MOCK_HAS_CLAIMS_CHANNEL.with_borrow_mut(|open| *open = true);
	MOCK_CLAIMS_MAX_MESSAGE_SIZE.with_borrow_mut(|size| *size = 100_000);
	runtime::new_test_ext()
}

/// The channel is `MockChannelInfo`'s to report, so the helper only sets its per-message room.
#[cfg(feature = "runtime-benchmarks")]
pub struct MockCreditsBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl crate::benchmarking::BenchmarkHelper for MockCreditsBenchmarkHelper {
	fn open_nft_claims_channel(max_message_size: u32) {
		// `MockChannelInfo` reports an open channel unless a test closes it; only its per-message
		// room has to be set, which is what decides how many trees a delivery takes.
		set_claims_max_message_size(max_message_size);
	}
}
