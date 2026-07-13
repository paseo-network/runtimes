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

//! Runtime API definition for the mob rule pallet.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use crate::{Alias, CaseIndex};
use alloc::vec::Vec;
use codec::Codec;

sp_api::decl_runtime_apis! {
	/// The API to query the rewards for mob credit.
	pub trait MobRuleApi<AccountId, Balance>
		where
		AccountId: Codec,
		Balance: Codec,
	{
		/// Returns a list of cases where the user has a vote stored on chain. If the `done_only`
		/// flag is set, only cases that are done and ready to be claimed will be returned. This
		/// function does not take the correctness of the vote into account.
		fn voted_on(voter: &Alias, done_only: bool) -> Vec<CaseIndex>;
	}
}
