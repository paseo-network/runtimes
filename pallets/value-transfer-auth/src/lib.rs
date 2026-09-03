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

//! # ⚠ PERMANENT PASEO FORK — NO UPSTREAM
//!
//! This pallet was vendored from `paritytech/individuality-community` at `28b7d07`
//! ("Initial public release") and **deleted upstream** at `e30decd` ("Public release
//! 2026-08-18"). It is absent from `v0.3.0` and `v0.3.1` and from every later tag.
//!
//! There is therefore **nothing to sync it against, ever**. It is Paseo-owned code with a
//! permanent maintenance burden. This matters more here than for
//! `indiv-pallet-storage-initialization`, because the Paseo deviation below is
//! **security-relevant and will never be reviewed upstream again**.
//!
//! ## Paseo-local deviations from the `28b7d07` vendored baseline
//!
//! | File | Deviation |
//! |---|---|
//! | `src/guarded_transactor.rs` | 🔴 **A cleared XCM origin (`context.origin == None`) is now PERMITTED** while the protected-asset block flag is set, in both `deposit_asset` and `deposit_asset_with_surplus`. The baseline rejected it (`.unwrap_or(false)`). The justification — recorded at both call sites — is that this is the post-`ClearOrigin` teleport-settlement shape, whose provenance was already enforced one instruction earlier at `ReceiveTeleportedAsset` by `IsTeleporter` / `ExternalAssetFromAssetHub`. A `Some(untrusted)` origin is still rejected. **Any change to Paseo's teleport filters invalidates that justification — re-audit this file when they change.** |
//! | `src/tests/guarded_transactor.rs` | tests retargeted from the `next-*` para ids (1500/1502) to Paseo's (AH 1000, People 1004), and the two rejection tests replaced with the cleared-origin acceptance tests that pin the behaviour above |
//!
//! ## Port status against individuality v0.3.1
//!
//! Compiles **unchanged** against v0.3.1's `indiv-support` and the v0.3.1 pallet set —
//! measured, not assumed (see `INDIVIDUALITY_C1_RESULT.md`, boundary matrix). It uses none
//! of the `MembershipProver` / `CurrentBlockRandomness` / `BatchProofItem` surface that the
//! v0.3.1 `support/src/traits.rs` reshapes.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod allow_only_siblings;
pub mod call_filter;
pub mod extension;
pub mod guarded_transactor;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub use allow_only_siblings::AllowOnlySiblings;
pub use call_filter::BlockValueTransfersWhenFlagSet;
pub use extension::{payload_hash, AuthorizeValueTransfer};
pub use guarded_transactor::ProtectedAssetTransactor;
