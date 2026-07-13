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

mod test_alias_proof_tampering;
mod test_big_endian_period_ordering;
mod test_bounded_vec_decoding;
mod test_coin_lifecycle_weight;
mod test_coinage_paid_full_story;
mod test_direct_offboard_coin_into_external_asset;
mod test_dusting;
mod test_extension;
mod test_free_unload_token_lifecycle;
mod test_infallible_unpaid_ext;
mod test_integrity;
mod test_load_recycler;
mod test_load_recycler_with_external_asset;
mod test_minimum_fee_coin_value_for_output_unload;
mod test_paid_ring_lifecycle;
mod test_pay_for_recycler_unload_fee_token_with_coin;
mod test_pay_for_recycler_unload_fee_token_with_external_asset;
mod test_pay_for_recycler_unload_fee_token_with_native;
mod test_recycler_lifecycle;
mod test_set_underlying_asset_id;
mod test_split;
mod test_transfer;
mod test_unload_recycler_into_coin;
mod test_unload_recycler_into_coins;
mod test_unload_recycler_into_external_asset;
mod test_unload_recycler_into_external_asset_and_vouchers;
mod test_unload_recycler_into_external_asset_fee_from_output;
mod test_unload_recycler_into_external_asset_non_anonymous_fee_from_signer;

// TODO:
// * tests for AsUnloadTokenPaid, AsUnloadTokenPeople, AsUnloadTokenLitePeople.
//
// Calls missing dedicated tests (potentially):
// * clean_recycler (call index 101)
// * clean_consumed_free_token (call index 102)
// * clean_paid_unload_token_ring (call index 104)
// * clean_recycler_dust (call index 105)
// * clean_paid_unload_token_dust (call index 106)
// * delete_expired_paid_unload_token_collection (call index 107)
