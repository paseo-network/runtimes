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
pub use pallet::*;
pub use types::*;

use super::*;
use frame_support::{
	pallet_prelude::*,
	traits::{EnsureOrigin, OriginTrait},
};
use frame_system::pallet_prelude::*;
use sp_runtime::traits::BadOrigin;

// ========== Origin Helper Functions ==========

/// Ensure that the origin `o` represents a ring alias in a specific collection.
/// Returns `Ok` with the alias on success.
pub fn ensure_ring_alias_of<OuterOrigin>(
	o: OuterOrigin,
	collection: &Identifier,
) -> Result<Alias, BadOrigin>
where
	OuterOrigin: TryInto<Origin, Error = OuterOrigin>,
{
	match o.try_into() {
		Ok(Origin::RingAlias(info)) if info.collection == *collection => Ok(info.ca.alias),
		_ => Err(BadOrigin),
	}
}

/// Ensure that the origin `o` represents a ring alias in a specific collection and context.
/// Returns `Ok` with the alias on success.
pub fn ensure_ring_alias_in_context<OuterOrigin>(
	o: OuterOrigin,
	collection: &Identifier,
	context: &Context,
) -> Result<Alias, BadOrigin>
where
	OuterOrigin: TryInto<Origin, Error = OuterOrigin>,
{
	match o.try_into() {
		Ok(Origin::RingAlias(info))
			if info.collection == *collection && info.ca.context == *context =>
			Ok(info.ca.alias),
		_ => Err(BadOrigin),
	}
}

// ========== EnsureOrigin Implementations ==========

/// Ensures the origin is a ring alias of a specific collection.
///
/// The collection is specified via the generic parameter `C: Get<Identifier>`.
/// Returns the member's alias on success.
pub struct EnsureRingAliasOf<T, C>(PhantomData<(T, C)>);

impl<T: Config, C: Get<Identifier>> EnsureOrigin<OriginFor<T>> for EnsureRingAliasOf<T, C> {
	type Success = Alias;

	fn try_origin(o: OriginFor<T>) -> Result<Self::Success, OriginFor<T>> {
		ensure_ring_alias_of(o.clone().into_caller(), &C::get()).map_err(|_| o)
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn try_successful_origin() -> Result<OriginFor<T>, ()> {
		Ok(Origin::RingAlias(AliasAccountInfo {
			collection: C::get(),
			revision: 0,
			ring: 0,
			ca: ContextualAlias { alias: [0u8; 32], context: [0u8; 32] },
		})
		.into())
	}
}

/// Ensures the origin is a ring alias of a specific collection and context.
///
/// Returns the member's alias on success.
pub struct EnsureRingAliasInContext<T, C, Ctx>(PhantomData<(T, C, Ctx)>);

impl<T: Config, C: Get<Identifier>, Ctx: Get<Context>> EnsureOrigin<OriginFor<T>>
	for EnsureRingAliasInContext<T, C, Ctx>
{
	type Success = Alias;

	fn try_origin(o: OriginFor<T>) -> Result<Self::Success, OriginFor<T>> {
		ensure_ring_alias_in_context(o.clone().into_caller(), &C::get(), &Ctx::get()).map_err(|_| o)
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn try_successful_origin() -> Result<OriginFor<T>, ()> {
		Ok(Origin::RingAlias(AliasAccountInfo {
			collection: C::get(),
			revision: 0,
			ring: 0,
			ca: ContextualAlias { alias: [0u8; 32], context: Ctx::get() },
		})
		.into())
	}
}

/// Ensures the origin is any ring alias, returning the full `AliasAccountInfo`.
pub struct EnsureRingAlias<T>(PhantomData<T>);

impl<T: Config> EnsureOrigin<OriginFor<T>> for EnsureRingAlias<T> {
	type Success = AliasAccountInfo;

	fn try_origin(o: OriginFor<T>) -> Result<Self::Success, OriginFor<T>> {
		match o.clone().into_caller().try_into() {
			Ok(Origin::RingAlias(info)) => Ok(info),
			_ => Err(o),
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn try_successful_origin() -> Result<OriginFor<T>, ()> {
		Ok(Origin::RingAlias(AliasAccountInfo {
			collection: [0u8; 32],
			revision: 0,
			ring: 0,
			ca: ContextualAlias { alias: [0u8; 32], context: [0u8; 32] },
		})
		.into())
	}
}
