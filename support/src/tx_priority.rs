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

//! Shared transaction-priority tiers for authorized calls (`#[pallet::authorize]`) and
//! origin-producing transaction extensions.
//!
//! Most of these transactions are feeless (special origin or `Pays::No`), so `ChargeAssetTxPayment`
//! is skipped via `SkipCheckIfFeeless` and they contribute no fee-based priority. Left unset, they
//! all land at `0` and the pool cannot order them under congestion.
//!
//! Setting `.priority(..)` in each `validate` / `authorize` body lets the pool rank them. Pipeline
//! priorities are summed, so the bands are spaced 1000x apart: summing many transactions within one
//! tier never reaches the next.
//!
//! # Tiers (lowest to highest)
//!
//! - [`CLEANUP`] — expired or stale data removal with no correctness dependency on timely inclusion
//!   (storage reclamation, dust collection). Yields to everything else.
//! - [`USER_DEFAULT`] — baseline user-authorized operations without a deadline (alias/identity
//!   setup, self-inclusion, resource registrations or claims, coinage operations).
//! - [`BACKGROUND_PROGRESS`] — deferrable automated progress that gates a user-visible transition
//!   or value movement (airdrop lifecycle steps, notifier batches, staged deletion/finalization).
//! - [`USER_HIGH`] — time-critical user operations bound by a deadline or window (e.g. casting
//!   votes, sign-ups during a registration window).
//! - [`PROTOCOL_LIVENESS`] — maintenance the protocol cannot make progress without (ring building,
//!   member onboarding, and the one-time bootstrap of core resources the rest of the system depends
//!   on, such as the people collection and the PGAS asset). Highest tier.
//!
//! `BACKGROUND_PROGRESS` sits below `USER_HIGH` so deadline-bound user work beats deferrable
//! maintenance; `PROTOCOL_LIVENESS` is highest because it covers protocol liveness.

use sp_runtime::transaction_validity::TransactionPriority;

/// Stale-data removal whose effect is freeing storage (reclamation, dust collection). Use this
/// when the step only frees storage, even if it also advances a state machine. Lowest tier.
pub const CLEANUP: TransactionPriority = 0;

/// Baseline priority for user-authorized operations that are not deadline-bound: alias/identity
/// setup, self-inclusion, resource registrations or claims, and coinage operations.
pub const USER_DEFAULT: TransactionPriority = 1_000;

/// Deferrable automated progress that gates a user-visible transition or value movement: airdrop
/// lifecycle steps, notifier batches, and staged deletion/finalization. Use this when delay holds
/// up a value transfer or user-facing transition; pure storage reclamation belongs in [`CLEANUP`],
/// and bootstrap that the whole protocol (or a subsystem) is blocked on belongs in
/// [`PROTOCOL_LIVENESS`].
pub const BACKGROUND_PROGRESS: TransactionPriority = 1_000_000;

/// Time-critical user operations bound by a deadline or window (e.g. casting votes, sign-ups).
pub const USER_HIGH: TransactionPriority = 1_000_000_000;

/// Liveness-critical maintenance: without it the protocol stalls (ring building, member
/// onboarding), plus the one-time bootstrap of core resources the rest of the system depends on
/// (the people collection that all membership work needs, the PGAS asset that all PGAS claims
/// need). Highest tier.
pub const PROTOCOL_LIVENESS: TransactionPriority = 1_000_000_000_000;

const _: () = {
	assert!(CLEANUP < USER_DEFAULT);
	assert!(USER_DEFAULT < BACKGROUND_PROGRESS);
	assert!(BACKGROUND_PROGRESS < USER_HIGH);
	assert!(USER_HIGH < PROTOCOL_LIVENESS);
};
