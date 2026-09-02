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

//! Benchmarking for the dotNS gateway pallet.

use super::*;
use codec::Encode;
use frame_benchmarking::v2::*;
use frame_system::RawOrigin as SystemOrigin;
use indiv_support::traits::{Identifier, RevisionIndex, RingIndex};
use sp_runtime::traits::{DispatchTransaction, TxBaseImplication};

pub trait BenchmarkHelper<T: Config> {
	/// Populates a ring root for the given collection identifier and ring index.
	fn setup_ring_root(identifier: &Identifier, ring_index: RingIndex) -> RevisionIndex;
	/// Returns a valid proof for the given collection, bound to the given message.
	fn valid_proof(collection: &Collection, message: &[u8]) -> ProofOf<T>;
	/// Returns the candidate account.
	fn candidate() -> T::AccountId;
	/// Returns a valid attestation signature over the given message from `candidate()`.
	fn sign(message: &[u8]) -> T::AttestationSignature;
	/// Sets the runtime's unix-time source to `seconds`.
	///
	/// Must be called with `seconds > 0` before any code path that reads
	/// `UnixTime::now()`; otherwise the runtime logs a "called at genesis" error.
	fn set_time(seconds: u64);
}

#[benchmarks(
	where
		T: Send + Sync,
		<T as frame_system::Config>::RuntimeCall:
			frame_support::traits::IsSubType<crate::pallet::Call<T>> + From<crate::pallet::Call<T>>,
		<<T as frame_system::Config>::RuntimeCall as sp_runtime::traits::Dispatchable>::Info:
			Default,
		<<T as frame_system::Config>::RuntimeCall as sp_runtime::traits::Dispatchable>::PostInfo:
			Default,
)]
mod benchmarks {
	use super::*;
	use crate::{AliasRegistration, AttestationAllowance, DispatcherAddress};
	use frame_support::traits::UnixTime;
	use frame_system::RawOrigin;
	use indiv_support::traits::PEOPLE_IDENTIFIER;
	use sp_core::H160;

	#[benchmark]
	fn reserve_name() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::set_time(1);
		let caller: T::AccountId = whitelisted_caller();
		// Allowance of 2 so after decrement the result is > 0, triggering the
		// `insert` path (heavier than `remove` when allowance reaches 0).
		AttestationAllowance::<T>::insert(&caller, 2u32);
		DispatcherAddress::<T>::put(H160([0xd0; 20]));

		// 32-byte lite-format label: 29 lowercase letters, a dot, two digits.
		// Matches `<dns-stem>.<2+ digits>` per `StringUtils.isLitePersonLabel`.
		let mut lite_bytes = [b'x'; 32];
		lite_bytes[29] = b'.';
		lite_bytes[30] = b'4';
		lite_bytes[31] = b'2';
		let lite_label = crate::BaseLabel::try_from(lite_bytes.to_vec())
			.map_err(|_| BenchmarkError::Stop("lite_label too long"))?;

		let chat_key = crate::ChatKey::from([b'k'; 65]);

		// 32-byte single DNS label (lowercase letters only).
		let reserved_base_label: Option<crate::BaseLabel> = Some(
			crate::BaseLabel::try_from([b'y'; 32].to_vec())
				.map_err(|_| BenchmarkError::Stop("reserved_base_label too long"))?,
		);

		let candidate = T::BenchmarkHelper::candidate();
		let signed_at = <T as crate::Config>::UnixTime::now().as_secs();
		let msg = crate::Pallet::<T>::construct_reservation_message(
			&candidate,
			&caller,
			lite_label.lite_base(),
			chat_key.as_bytes(),
			reserved_base_label.as_ref().map(crate::BaseLabel::as_slice),
			signed_at,
		);
		let signature = T::BenchmarkHelper::sign(&msg);

		#[extrinsic_call]
		_(
			RawOrigin::Signed(caller),
			candidate,
			signature,
			lite_label,
			chat_key,
			reserved_base_label,
			signed_at,
		);

		assert_eq!(crate::AccountNames::<T>::iter().count(), 1);

		Ok(())
	}

	#[benchmark]
	fn register_name() -> Result<(), BenchmarkError> {
		let caller: T::AccountId = whitelisted_caller();
		// 32-byte single DNS label for the full-person label position.
		let label = crate::BaseLabel::try_from([b'x'; 32].to_vec())
			.map_err(|_| BenchmarkError::Stop("label too long"))?;

		let mut lite_bytes = [b'l'; 32];
		lite_bytes[29] = b'.';
		lite_bytes[30] = b'4';
		lite_bytes[31] = b'2';
		let lite_label = crate::BaseLabel::try_from(lite_bytes.to_vec())
			.map_err(|_| BenchmarkError::Stop("lite_label too long"))?;
		let link = crate::Link::LiteUsername(lite_label.clone());

		crate::LiteLabelOwner::<T>::insert(&lite_label, &caller);
		DispatcherAddress::<T>::put(H160([0xd0; 20]));

		// Construct the custom origin directly. The extension is benchmarked
		// separately via `as_register_full_name_tx_ext`.
		let alias: indiv_support::traits::Alias = [7u8; 32];
		let origin: <T as frame_system::Config>::RuntimeOrigin =
			crate::pallet::Origin::PersonRegistration(alias).into();

		#[extrinsic_call]
		register_name(origin, caller, label, link);

		assert_eq!(AliasRegistration::<T>::iter().count(), 1);
		assert_eq!(crate::AccountNames::<T>::iter().count(), 1);

		Ok(())
	}

	#[benchmark]
	fn as_register_full_name_tx_ext() -> Result<(), BenchmarkError> {
		T::BenchmarkHelper::set_time(1);
		let revision = T::BenchmarkHelper::setup_ring_root(PEOPLE_IDENTIFIER, 0);
		let caller: T::AccountId = T::BenchmarkHelper::candidate();
		// 32-byte single DNS label for the full-person label position.
		let label = crate::BaseLabel::try_from([b'x'; 32].to_vec())
			.map_err(|_| BenchmarkError::Stop("label too long"))?;

		let mut lite_bytes = [b'l'; 32];
		lite_bytes[29] = b'.';
		lite_bytes[30] = b'4';
		lite_bytes[31] = b'2';
		let lite_label = crate::BaseLabel::try_from(lite_bytes.to_vec())
			.map_err(|_| BenchmarkError::Stop("lite_label too long"))?;
		let link = crate::Link::LiteUsername(lite_label.clone());

		crate::LiteLabelOwner::<T>::insert(&lite_label, &caller);
		DispatcherAddress::<T>::put(H160([0xd0; 20]));

		let proof_msg =
			crate::Pallet::<T>::construct_register_proof_message(&caller, label.as_slice(), &link);
		let proof = T::BenchmarkHelper::valid_proof(&Collection::People, &proof_msg);

		let call = crate::pallet::Call::<T>::register_name { who: caller.clone(), label, link };
		let runtime_call: <T as frame_system::Config>::RuntimeCall = call.into();
		let len = runtime_call.encode().len();
		let extension_version = 0u8;

		// No other extension in our bench, this is the `inherited_implication`.
		let sig_msg = TxBaseImplication((extension_version, &runtime_call))
			.using_encoded(sp_io::hashing::blake2_256);
		let signature = T::BenchmarkHelper::sign(&sig_msg);

		let tx_ext =
			crate::AsDotnsGateway::<T>::new(Some(crate::AsDotnsGatewayInfo::RegisterFullName {
				proof,
				ring_index: 0,
				revision,
				signature,
			}));

		#[block]
		{
			tx_ext
				.test_run(
					SystemOrigin::None.into(),
					&runtime_call,
					&Default::default(),
					len,
					extension_version,
					|_| Ok(Default::default()),
				)
				.map_err(|_| BenchmarkError::Stop("test_run failed"))?
				.map_err(|_| BenchmarkError::Stop("dispatch failed"))?;
		}

		Ok(())
	}

	#[benchmark]
	fn increase_attestation_allowance() -> Result<(), BenchmarkError> {
		let account: T::AccountId = whitelisted_caller();
		// Pre-populating so the storage read hits an existing entry (larger PoV
		// than reading a default-zero absent key).
		AttestationAllowance::<T>::insert(&account, 5u32);

		#[extrinsic_call]
		_(RawOrigin::Root, account.clone(), 10u32);

		assert_eq!(AttestationAllowance::<T>::get(&account), 15);

		Ok(())
	}

	#[benchmark]
	fn clear_attestation_allowance() -> Result<(), BenchmarkError> {
		let account: T::AccountId = whitelisted_caller();
		AttestationAllowance::<T>::insert(&account, 10u32);

		#[extrinsic_call]
		_(RawOrigin::Root, account.clone());

		assert_eq!(AttestationAllowance::<T>::get(&account), 0);

		Ok(())
	}

	#[benchmark]
	fn set_dispatcher_address() -> Result<(), BenchmarkError> {
		// Storage already populated, so the call overwrites an existing entry.
		DispatcherAddress::<T>::put(H160([0x11; 20]));
		let new_addr = H160([0x22; 20]);

		#[extrinsic_call]
		_(RawOrigin::Root, new_addr);

		assert_eq!(DispatcherAddress::<T>::get(), Some(new_addr));

		Ok(())
	}

	impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
