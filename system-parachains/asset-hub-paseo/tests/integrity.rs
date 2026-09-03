// Copyright (C) Parity Technologies (UK) Ltd.
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

//! Runs the runtime's aggregate `integrity_test`.
//!
//! FRAME collects every pallet's `#[pallet::hooks] fn integrity_test` into
//! `AllPalletsWithSystem::integrity_test()`. Those assertions are the runtime's own statement of
//! which `Config` combinations are coherent, and several of them are exactly the kind of thing
//! that compiles cleanly and then panics a collator at startup or misbehaves silently in
//! production. Nothing in this repo invoked them before; this target does.
//!
//! Deliberately a SEPARATE test target from `tests.rs`: those binaries do not currently compile
//! on this branch (nor on the base branch), and a separate target compiles independently.

use asset_hub_paseo_runtime::AllPalletsWithSystem;
use frame_support::traits::IntegrityTest;

#[test]
fn runtime_integrity_test() {
	sp_io::TestExternalities::default().execute_with(|| {
		AllPalletsWithSystem::integrity_test();
	});
}
