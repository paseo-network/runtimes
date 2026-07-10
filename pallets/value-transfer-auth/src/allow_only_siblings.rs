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

use core::marker::PhantomData;
use frame_support::traits::{Contains, Get};
use xcm::latest::{Junction::Parachain, Location};

pub struct AllowOnlySiblings<A, B>(PhantomData<(A, B)>);

impl<A: Get<u32>, B: Get<u32>> Contains<Location> for AllowOnlySiblings<A, B> {
	fn contains(loc: &Location) -> bool {
		match loc.unpack() {
			(1, [Parachain(id)]) => *id == A::get() || *id == B::get(),
			_ => false,
		}
	}
}
