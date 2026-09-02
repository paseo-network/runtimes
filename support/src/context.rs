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

//! Product-scoped ring VRF context construction.

use alloc::vec::Vec;

use crate::traits::Context;
use frame_support::{traits::ConstU32, BoundedVec};

const PRODUCT_PREFIX: [u8; 8] = *b"product/";

/// Maximum length of the network suffix appended to a product identifier.
pub const MAX_NETWORK_SUFFIX_LENGTH: u32 = 16;

/// Network suffix appended to product identifiers when constructing ring VRF contexts.
pub type ProductContextNetworkSuffix = BoundedVec<u8, ConstU32<MAX_NETWORK_SUFFIX_LENGTH>>;

/// The suffix appended to a product identifier when constructing a ring VRF context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProductContextSuffix {
	/// An enumerable suffix expanded according to
	/// [truAPI RFC-0022](https://github.com/paritytech/truapi/blob/main/docs/rfcs/0022-account-derivations.md).
	Index(u32),
	/// A suffix that is already represented as 32 bytes.
	Raw([u8; 32]),
}

impl ProductContextSuffix {
	fn bytes(self) -> [u8; 32] {
		match self {
			Self::Index(index) => {
				let mut bytes = [0u8; 32];
				bytes[..4].copy_from_slice(&index.to_le_bytes());
				bytes[4..].copy_from_slice(&INDEX_MAGIC);
				bytes
			},
			Self::Raw(bytes) => bytes,
		}
	}
}

/// The trailing marker in an expanded enumerable suffix.
///
/// Its value is `blake2b_256("product-account-index")[..28]`, as defined by truAPI RFC-0022.
const INDEX_MAGIC: [u8; 28] = [
	0x12, 0xe8, 0x60, 0x13, 0x73, 0x6c, 0x54, 0x98, 0xf0, 0x50, 0xb0, 0x3c, 0xdc, 0x16, 0x95, 0x7d,
	0xff, 0x0e, 0x42, 0x2f, 0xb9, 0x2c, 0xa7, 0x7e, 0xc3, 0xab, 0x16, 0x8f,
];

/// Build a context for `product_name.network_suffix`.
///
/// The preimage is `"product/" ++ product_name ++ "." ++ network_suffix ++ "/" ++
/// suffix.bytes()`.
pub fn build_product_context(
	product_name: &[u8],
	network_suffix: &[u8],
	suffix: ProductContextSuffix,
) -> Context {
	let suffix = suffix.bytes();
	let mut input =
		Vec::with_capacity(
			PRODUCT_PREFIX.len() +
				product_name.len() +
				b".".len() + network_suffix.len() +
				b"/".len() + suffix.len(),
		);
	input.extend_from_slice(&PRODUCT_PREFIX);
	input.extend_from_slice(product_name);
	input.push(b'.');
	input.extend_from_slice(network_suffix);
	input.push(b'/');
	input.extend_from_slice(&suffix);
	sp_crypto_hashing::blake2_256(&input)
}

/// Context suffix allocations owned by the personhood product.
pub mod personhood {
	use super::ProductContextSuffix;

	/// The personhood product name shared by all networks.
	pub const PRODUCT_NAME: &[u8] = b"peopl";
	/// Score's enumerable context allocation.
	pub const SCORE: ProductContextSuffix = ProductContextSuffix::Index(0);
	/// Resources' enumerable context allocation.
	pub const RESOURCES: ProductContextSuffix = ProductContextSuffix::Index(1);
	/// People-lite authentication's enumerable context allocation.
	pub const PEOPLE_LITE_AUTH: ProductContextSuffix = ProductContextSuffix::Index(2);
	/// The dotNS gateway's enumerable context allocation.
	pub const DOTNS_GATEWAY: ProductContextSuffix = ProductContextSuffix::Index(3);
	/// People Airdrops' enumerable context allocation.
	pub const PEOPLE_AIRDROPS: ProductContextSuffix = ProductContextSuffix::Index(4);

	const SYSTEM_SUFFIX_PREFIX: [u8; 4] = *b"sys/";
	const RESOURCES_NOTIFICATION_FAMILY: u32 = 1;
	const STATEMENT_STORE_SLOT_FAMILY: u32 = 2;
	const LONG_TERM_STORAGE_FAMILY: u32 = 3;
	const PGAS_CLAIM_FAMILY: u32 = 4;

	/// Build `Raw("sys/" ++ u32_le(1) ++ u32_le(period) ++ seq ++ zero padding)`.
	pub fn resources_notification(period: u32, seq: u8) -> ProductContextSuffix {
		raw_u32_u8(RESOURCES_NOTIFICATION_FAMILY, period, seq)
	}

	/// Build `Raw("sys/" ++ u32_le(2) ++ u32_le(period) ++ u32_le(seq) ++ zero padding)`.
	pub fn statement_store_slot(period: u32, seq: u32) -> ProductContextSuffix {
		raw_u32_u32(STATEMENT_STORE_SLOT_FAMILY, period, seq)
	}

	/// Build `Raw("sys/" ++ u32_le(3) ++ u32_le(period) ++ counter ++ zero padding)`.
	pub fn long_term_storage(period: u32, counter: u8) -> ProductContextSuffix {
		raw_u32_u8(LONG_TERM_STORAGE_FAMILY, period, counter)
	}

	/// Build `Raw("sys/" ++ u32_le(4) ++ u32_le(day) ++ u32_le(slot) ++ zero padding)`.
	pub fn pgas_claim(day: u32, slot: u32) -> ProductContextSuffix {
		raw_u32_u32(PGAS_CLAIM_FAMILY, day, slot)
	}

	fn raw_u32_u8(family: u32, value: u32, tail: u8) -> ProductContextSuffix {
		let mut suffix = [0u8; 32];
		suffix[..4].copy_from_slice(&SYSTEM_SUFFIX_PREFIX);
		suffix[4..8].copy_from_slice(&family.to_le_bytes());
		suffix[8..12].copy_from_slice(&value.to_le_bytes());
		suffix[12] = tail;
		ProductContextSuffix::Raw(suffix)
	}

	fn raw_u32_u32(family: u32, first: u32, second: u32) -> ProductContextSuffix {
		let mut suffix = [0u8; 32];
		suffix[..4].copy_from_slice(&SYSTEM_SUFFIX_PREFIX);
		suffix[4..8].copy_from_slice(&family.to_le_bytes());
		suffix[8..12].copy_from_slice(&first.to_le_bytes());
		suffix[12..16].copy_from_slice(&second.to_le_bytes());
		ProductContextSuffix::Raw(suffix)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn index_suffix_uses_little_endian_index_and_magic() {
		assert_eq!(
			ProductContextSuffix::Index(0x0102_0304).bytes(),
			[
				0x04, 0x03, 0x02, 0x01, 0x12, 0xe8, 0x60, 0x13, 0x73, 0x6c, 0x54, 0x98, 0xf0, 0x50,
				0xb0, 0x3c, 0xdc, 0x16, 0x95, 0x7d, 0xff, 0x0e, 0x42, 0x2f, 0xb9, 0x2c, 0xa7, 0x7e,
				0xc3, 0xab, 0x16, 0x8f,
			]
		);
	}

	#[test]
	fn index_magic_matches_rfc_derivation() {
		assert_eq!(INDEX_MAGIC, sp_crypto_hashing::blake2_256(b"product-account-index")[..28]);
	}

	#[test]
	fn raw_suffix_is_used_verbatim() {
		let raw = core::array::from_fn(|index| index as u8);

		assert_eq!(ProductContextSuffix::Raw(raw).bytes(), raw);
	}

	#[test]
	fn indexed_suffix_and_its_raw_representation_are_identical() {
		let indexed = ProductContextSuffix::Index(7).bytes();

		assert_eq!(indexed, ProductContextSuffix::Raw(indexed).bytes());
	}

	#[test]
	fn network_suffix_is_appended_to_product_name() {
		let suffix = ProductContextSuffix::Index(0).bytes();
		let mut expected_preimage = b"product/voting.dot/".to_vec();
		expected_preimage.extend_from_slice(&suffix);

		assert_eq!(
			build_product_context(b"voting", b"dot", ProductContextSuffix::Index(0)),
			sp_crypto_hashing::blake2_256(&expected_preimage),
		);
	}

	#[test]
	fn personhood_allocations_are_stable() {
		assert_eq!(personhood::SCORE, ProductContextSuffix::Index(0));
		assert_eq!(personhood::RESOURCES, ProductContextSuffix::Index(1));
		assert_eq!(personhood::PEOPLE_LITE_AUTH, ProductContextSuffix::Index(2));
		assert_eq!(personhood::DOTNS_GATEWAY, ProductContextSuffix::Index(3));
		assert_eq!(personhood::PEOPLE_AIRDROPS, ProductContextSuffix::Index(4));
	}

	#[test]
	fn raw_suffix_is_appended_to_product_preimage() {
		let raw = core::array::from_fn(|index| index as u8);
		let mut expected_preimage = b"product/voting.dot/".to_vec();
		expected_preimage.extend_from_slice(&raw);

		assert_eq!(
			build_product_context(b"voting", b"dot", ProductContextSuffix::Raw(raw)),
			sp_crypto_hashing::blake2_256(&expected_preimage),
		);
	}

	#[test]
	fn personhood_parameterized_suffixes_use_allocated_layouts() {
		fn u32_at(bytes: &[u8; 32], offset: usize) -> u32 {
			u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("four-byte field"))
		}

		let period = 0x0102_0304;
		let sequence = 0x0506_0708;

		let notification = personhood::resources_notification(period, 5).bytes();
		assert_eq!(&notification[..4], b"sys/");
		assert_eq!(u32_at(&notification, 4), 1);
		assert_eq!(u32_at(&notification, 8), period);
		assert_eq!(notification[12], 5);
		assert!(notification[13..].iter().all(|byte| *byte == 0));

		let statement_slot = personhood::statement_store_slot(period, sequence).bytes();
		assert_eq!(&statement_slot[..4], b"sys/");
		assert_eq!(u32_at(&statement_slot, 4), 2);
		assert_eq!(u32_at(&statement_slot, 8), period);
		assert_eq!(u32_at(&statement_slot, 12), sequence);
		assert!(statement_slot[16..].iter().all(|byte| *byte == 0));

		let long_term_storage = personhood::long_term_storage(period, 5).bytes();
		assert_eq!(&long_term_storage[..4], b"sys/");
		assert_eq!(u32_at(&long_term_storage, 4), 3);
		assert_eq!(u32_at(&long_term_storage, 8), period);
		assert_eq!(long_term_storage[12], 5);
		assert!(long_term_storage[13..].iter().all(|byte| *byte == 0));

		let pgas_claim = personhood::pgas_claim(period, sequence).bytes();
		assert_eq!(&pgas_claim[..4], b"sys/");
		assert_eq!(u32_at(&pgas_claim, 4), 4);
		assert_eq!(u32_at(&pgas_claim, 8), period);
		assert_eq!(u32_at(&pgas_claim, 12), sequence);
		assert!(pgas_claim[16..].iter().all(|byte| *byte == 0));
	}
}
