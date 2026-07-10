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

//! `BaseCallFilter` adapter gating value-transfer calls on the global block flag.

use crate::extension::block_flag;
use core::marker::PhantomData;
use frame_support::traits::Contains;

/// `Contains<Call>` adapter that rejects matched value-transfer calls while the global block
/// flag is set.
///
/// Wire this into `frame_system::Config::BaseCallFilter`.
pub struct BlockValueTransfersWhenFlagSet<ValueTransfers>(PhantomData<ValueTransfers>);

impl<Call, ValueTransfers> Contains<Call> for BlockValueTransfersWhenFlagSet<ValueTransfers>
where
	ValueTransfers: Contains<Call>,
{
	fn contains(call: &Call) -> bool {
		!ValueTransfers::contains(call) || !block_flag::is_blocked()
	}
}
