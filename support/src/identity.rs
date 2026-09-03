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

use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

use crate::traits::Alias;

/// An account or a person.
#[derive(
	PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen, DecodeWithMemTracking,
)]
pub enum AccountOrPerson<AccountId> {
	/// An account.
	Account(AccountId),
	/// A person.
	Person(Alias),
}

impl<AccountId> AccountOrPerson<AccountId> {
	/// Get the account if it is an account.
	pub fn account(&self) -> Option<&AccountId> {
		match &self {
			AccountOrPerson::Account(account) => Some(account),
			AccountOrPerson::Person(_) => None,
		}
	}
}
