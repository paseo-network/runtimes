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

//! The fixed-address collection factory precompile.

use super::*;
use pallet_scarcity::{weights::WeightInfo as _, Pallet as Scarcity};
use IScarcityFactory::IScarcityFactoryCalls;

/// Collection factory at the fixed address index `INDEX`.
///
/// Creates collections owned by the caller. The returned id names the collection's own
/// per-collection precompile address; see the crate documentation for the layout.
pub struct ScarcityFactory<T, const INDEX: u16>(PhantomData<T>);

impl<T, const INDEX: u16> pallet_revive::precompiles::Precompile for ScarcityFactory<T, INDEX>
where
	T: pallet_scarcity::Config + pallet_revive::Config,
{
	type T = T;
	type Interface = IScarcityFactoryCalls;
	const MATCHER: AddressMatcher = AddressMatcher::Fixed(NonZero::new(INDEX).unwrap());
	const HAS_CONTRACT_INFO: bool = false;

	fn call(
		_address: &[u8; 20],
		input: &Self::Interface,
		env: &mut impl Ext<T = Self::T>,
	) -> Result<Vec<u8>, Error> {
		frame_support::ensure!(
			!env.is_delegate_call(),
			pallet_revive::Error::<T>::PrecompileDelegateDenied
		);
		ensure_no_value(env)?;
		if env.is_read_only() {
			return Err(Error::Error(pallet_revive::Error::<T>::StateChangeDenied.into()));
		}

		match input {
			IScarcityFactoryCalls::createCollection(_) => {
				env.charge(<T as pallet_scarcity::Config>::WeightInfo::create_collection())?;
				let who = caller_account::<T>(env)?;
				let collection = Scarcity::<T>::do_create_collection(who.clone())
					.map_err(revert_scarcity::<T>)?;
				deposit_event(
					env,
					IScarcityFactory::CollectionCreated {
						collection,
						owner: address_of::<T>(&who),
					},
				)?;
				Ok(IScarcityFactory::createCollectionCall::abi_encode_returns(&collection))
			},
		}
	}
}
