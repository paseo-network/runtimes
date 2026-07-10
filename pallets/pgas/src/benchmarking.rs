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

//! PGAS pallet benchmarks.

#![allow(unused)]

use super::*;
use crate::{extension::AsPgas, Pallet as Pgas};
use codec::Encode;
use core::time::Duration;
use frame_benchmarking::v2::{benchmarks, *};
use frame_support::{
	dispatch::{DispatchInfo, GetDispatchInfo, PostDispatchInfo},
	traits::{
		fungibles::{Create, Inspect},
		Get, IsSubType,
	},
};
use frame_system::{offchain::CreateAuthorizedTransaction, RawOrigin as SystemOrigin};
use indiv_support::traits::{Identifier, RingIndex};
use sp_runtime::traits::{
	AsTransactionAuthorizedOrigin, DispatchTransaction, Dispatchable, TxBaseImplication,
};
use verifiable::GenerateVerifiable;

/// The `GenerateVerifiable` impl behind the runtime's configured [`MembershipProver`].
pub type CryptoOf<T> = <<T as Config>::MembershipProver as MembershipProver>::Crypto;
/// Convenience alias for the prover's `Member` type.
pub type MemberOf<T> = <CryptoOf<T> as GenerateVerifiable>::Member;
/// Convenience alias for the prover's `Secret` type.
pub type SecretOf<T> = <CryptoOf<T> as GenerateVerifiable>::Secret;

/// Benchmark helper trait.
pub trait BenchmarkHelper<T: Config> {
	/// Sets a time in seconds since the UNIX epoch for benchmarks.
	fn set_time(now: Duration);

	/// Seed the membership prover at `(identifier, ring_index)` with at least one member and
	/// return a [`ProofOf<T>`] that will verify against `context` + `message` for that ring.
	///
	/// Runtimes are free to implement this however is most convenient — by pushing a ring root
	/// into `pallet-members-subscriber`'s storage, by reusing a pre-computed proof cache, etc.
	/// The pallet benchmark calls this once to produce a valid setup and then measures the
	/// extension's validate/prepare/post_dispatch path around the returned proof.
	fn seed_and_create_proof(
		identifier: &Identifier,
		ring_index: RingIndex,
		context: &Context,
		message: &[u8],
	) -> ProofOf<T>;
}

#[benchmarks(
	where
		<T as frame_system::Config>::RuntimeCall:
			Dispatchable<Info = DispatchInfo, PostInfo = PostDispatchInfo>
				+ IsSubType<Call<T>>
				+ From<Call<T>>
				+ GetDispatchInfo
				+ Encode,
		<T as frame_system::Config>::RuntimeOrigin: AsTransactionAuthorizedOrigin,
)]
mod benches {
	use super::*;
	use crate::{extension::AsPgasInfo, pallet::Day};

	#[benchmark]
	fn claim_pgas() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::set_time(Duration::from_secs(86400));

		let admin: T::AccountId = account("admin", 0, 0);
		frame_system::Pallet::<T>::inc_sufficients(&admin);
		T::Fungibles::create(T::PgasAssetId::get(), admin, true, T::PgasMinBalance::get())
			.expect("asset creation should work");

		let alias: Alias = [0x42u8; 32];
		let day = Day::from(1u32);
		let origin: T::RuntimeOrigin =
			crate::Origin::ClaimAlias { alias, day, collection: PgasCollection::People }.into();

		let target: T::AccountId = account("target", 0, 0);

		#[extrinsic_call]
		_(origin, 0u32, target.clone());

		assert!(ClaimedGasAliases::<T>::contains_key(day, alias));
		Ok(())
	}

	#[benchmark]
	fn create_pgas_asset() -> Result<(), BenchmarkError> {
		#[extrinsic_call]
		_(SystemOrigin::Authorized);

		assert!(T::Fungibles::asset_exists(T::PgasAssetId::get()));
		Ok(())
	}

	#[benchmark]
	fn authorize_create_pgas_asset() -> Result<(), BenchmarkError> {
		#[block]
		{
			Pgas::<T>::authorize_create_pgas_asset().expect("authorize should succeed");
		}
		Ok(())
	}

	#[benchmark]
	fn clean_pgas_claim_records(
		n: Linear<1, { T::MaxPgasClaimRecordCleanupPerCall::get() }>,
	) -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::set_time(Duration::from_secs(86400));

		let day_index = 0u32;
		let day = Day::from(day_index);

		for i in 0..n {
			let mut alias: Alias = [0u8; 32];
			alias[0..4].copy_from_slice(&i.to_le_bytes());
			ClaimedGasAliases::<T>::insert(day, alias, ());
		}

		assert_eq!(ClaimedGasAliases::<T>::iter_prefix(day).count(), n as usize);

		let first_alias = ClaimedGasAliases::<T>::iter_key_prefix(day)
			.next()
			.expect("at least one record inserted above");

		#[extrinsic_call]
		_(SystemOrigin::Authorized, day_index, first_alias);

		assert!(ClaimedGasAliases::<T>::iter_prefix(day).next().is_none());
		Ok(())
	}

	#[benchmark]
	fn authorize_clean_pgas_claim_records() -> Result<(), BenchmarkError> {
		use sp_runtime::transaction_validity::TransactionSource;
		T::BenchmarkHelper::set_time(Duration::from_secs(86400 * 2));
		let day_index = 0u32;
		let day = Day::from(day_index);
		let alias: Alias = [0u8; 32];
		ClaimedGasAliases::<T>::insert(day, alias, ());

		let first_alias = ClaimedGasAliases::<T>::iter_key_prefix(day)
			.next()
			.expect("at least one record inserted above");

		#[block]
		{
			Pgas::<T>::authorize_clean_pgas_claim_records(
				TransactionSource::Local,
				day_index,
				first_alias,
			)
			.expect("authorize should succeed");
		}

		Ok(())
	}

	/// Weight of the [`AsPgas`] transaction extension for a successful people-collection claim.
	#[benchmark]
	fn as_pgas_claim_tx_ext() -> Result<(), BenchmarkError> {
		// Middle of the period.
		let now = Duration::from_secs(SECS_PER_DAY + SECS_PER_DAY / 2);
		T::BenchmarkHelper::set_time(now);

		// Asset must exist.
		let admin: T::AccountId = account("pgas_admin", 0, 0);
		frame_system::Pallet::<T>::inc_sufficients(&admin);
		T::Fungibles::create(T::PgasAssetId::get(), admin, true, T::PgasMinBalance::get())
			.expect("asset create should succeed");

		let target: T::AccountId = whitelisted_caller();
		let slot_index = 0u32;
		let call = Call::<T>::claim_pgas { slot_index, target };
		let call: <T as frame_system::Config>::RuntimeCall = call.into();
		let extension_version = 0u8;

		// Match the extension's on-chain message derivation exactly.
		let msg =
			TxBaseImplication((extension_version, &call)).using_encoded(sp_io::hashing::blake2_256);
		let day = Pgas::<T>::current_day();
		let context = Pgas::<T>::build_gas_context(day, slot_index);
		let identifier = PgasCollection::People.identifier();
		let ring_index = 0u32;

		let proof =
			T::BenchmarkHelper::seed_and_create_proof(&identifier, ring_index, &context, &msg);
		// Whatever revision the helper seeded — we measure the happy path where the caller
		// names the correct current revision.
		let revision =
			<T::MembershipProver as MembershipProver>::ring_revision(&identifier, ring_index)
				.expect("`seed_and_create_proof` must leave a ring in place");

		let tx_ext = AsPgas::<T>::new(Some(AsPgasInfo::Claim {
			proof,
			ring_index,
			revision,
			collection: PgasCollection::People,
			day,
		}));
		let info = call.get_dispatch_info();
		let len = call.encoded_size();

		#[block]
		{
			tx_ext
				.test_run(SystemOrigin::None.into(), &call, &info, len, extension_version, |_| {
					Ok(Default::default())
				})
				.expect("test_run must produce a result")
				.expect("dispatch substitute must succeed");
		}

		Ok(())
	}

	impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
