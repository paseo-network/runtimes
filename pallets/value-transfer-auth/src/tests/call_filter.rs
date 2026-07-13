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

use crate::{call_filter::BlockValueTransfersWhenFlagSet, extension::block_flag};
use frame_support::traits::{Contains, Everything, Nothing};

#[test]
fn flag_defaults_to_blocked() {
	block_flag::block();
	assert!(block_flag::is_blocked());
}

#[test]
fn non_value_calls_pass_when_blocked() {
	block_flag::block();
	assert!(<BlockValueTransfersWhenFlagSet<Nothing> as Contains<u32>>::contains(&0));
}

#[test]
fn value_calls_blocked_when_flag_set() {
	block_flag::block();
	assert!(!<BlockValueTransfersWhenFlagSet<Everything> as Contains<u32>>::contains(&0));
}

#[test]
fn value_calls_allowed_when_flag_unblocked() {
	block_flag::unblock();
	assert!(<BlockValueTransfersWhenFlagSet<Everything> as Contains<u32>>::contains(&0));
	block_flag::block();
}
