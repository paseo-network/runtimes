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

use codec::{Decode, DecodeWithMemTracking, Encode};
use frame_support::{
	traits::Get, weights::Weight, CloneNoBound, DebugNoBound, DefaultNoBound, EqNoBound,
	PartialEqNoBound,
};
use scale_info::TypeInfo;
use sp_runtime::{
	traits::{
		DispatchInfoOf, Implication, PostDispatchInfoOf, TransactionExtension, ValidateResult,
	},
	transaction_validity::{InvalidTransaction, TransactionValidityError, ValidTransaction},
};

/// Storage:
/// - `std` builds (`cargo test` native runner) use `thread_local!` so each test thread has its own
///   flag — avoids the data race that `cargo test`'s parallel scheduler triggers on `static mut`.
/// - `no_std` builds (the runtime executing inside Wasm) keep a `static mut`. Wasm executes
///   single-threaded, so the unsynchronized access is sound there.
pub mod block_flag {
	#[cfg(feature = "std")]
	std::thread_local! {
		static VALUE_TRANSFERS_BLOCKED: core::cell::Cell<bool> = const { core::cell::Cell::new(true) };
	}

	#[cfg(not(feature = "std"))]
	static mut VALUE_TRANSFERS_BLOCKED: bool = true;

	pub fn block() {
		#[cfg(feature = "std")]
		VALUE_TRANSFERS_BLOCKED.with(|c| c.set(true));
		#[cfg(not(feature = "std"))]
		unsafe {
			VALUE_TRANSFERS_BLOCKED = true;
		}
	}

	pub fn unblock() {
		#[cfg(feature = "std")]
		VALUE_TRANSFERS_BLOCKED.with(|c| c.set(false));
		#[cfg(not(feature = "std"))]
		unsafe {
			VALUE_TRANSFERS_BLOCKED = false;
		}
	}

	pub fn is_blocked() -> bool {
		#[cfg(feature = "std")]
		{
			VALUE_TRANSFERS_BLOCKED.with(|c| c.get())
		}
		#[cfg(not(feature = "std"))]
		{
			unsafe { VALUE_TRANSFERS_BLOCKED }
		}
	}
}

#[derive(
	Encode,
	Decode,
	DecodeWithMemTracking,
	DefaultNoBound,
	EqNoBound,
	PartialEqNoBound,
	CloneNoBound,
	DebugNoBound,
	TypeInfo,
)]
#[scale_info(skip_type_params(T, P))]
pub struct AuthorizeValueTransfer<T, P>(
	pub Option<sp_core::ed25519::Signature>,
	#[codec(skip)] pub core::marker::PhantomData<(T, P)>,
);

pub fn payload_hash<I: codec::Encode + ?Sized>(inherited_implication: &I) -> [u8; 32] {
	inherited_implication.using_encoded(sp_io::hashing::blake2_256)
}

impl<T, P> TransactionExtension<<T as frame_system::Config>::RuntimeCall>
	for AuthorizeValueTransfer<T, P>
where
	T: frame_system::Config + Send + Sync,
	P: Get<sp_core::ed25519::Public> + Send + Sync + 'static,
{
	const IDENTIFIER: &'static str = "AuthorizeValueTransfer";
	type Implicit = ();

	type Val = bool;
	type Pre = bool;

	fn weight(&self, _call: &<T as frame_system::Config>::RuntimeCall) -> Weight {
		if self.0.is_none() {
			return Weight::zero();
		}

		Weight::from_parts(50_000_000, 0)
	}

	fn validate(
		&self,
		origin: <T as frame_system::Config>::RuntimeOrigin,
		_call: &<T as frame_system::Config>::RuntimeCall,
		_info: &DispatchInfoOf<<T as frame_system::Config>::RuntimeCall>,
		_len: usize,
		_self_implicit: Self::Implicit,
		inherited_implication: &impl Implication,
		_source: frame_support::pallet_prelude::TransactionSource,
	) -> ValidateResult<Self::Val, <T as frame_system::Config>::RuntimeCall> {
		let Some(sig) = self.0.as_ref() else {
			return Ok((ValidTransaction::default(), false, origin));
		};

		let payload_hash_bytes = payload_hash(inherited_implication);
		if !sp_io::crypto::ed25519_verify(sig, &payload_hash_bytes, &P::get()) {
			return Err(InvalidTransaction::BadProof.into());
		}

		Ok((ValidTransaction::default(), true, origin))
	}

	fn prepare(
		self,
		val: Self::Val,
		_origin: &<T as frame_system::Config>::RuntimeOrigin,
		_call: &<T as frame_system::Config>::RuntimeCall,
		_info: &DispatchInfoOf<<T as frame_system::Config>::RuntimeCall>,
		_len: usize,
	) -> Result<Self::Pre, TransactionValidityError> {
		if val {
			block_flag::unblock();
		}
		Ok(val)
	}

	fn post_dispatch_details(
		pre: Self::Pre,
		_info: &DispatchInfoOf<<T as frame_system::Config>::RuntimeCall>,
		_post_info: &PostDispatchInfoOf<<T as frame_system::Config>::RuntimeCall>,
		_len: usize,
		_result: &sp_runtime::DispatchResult,
	) -> Result<Weight, TransactionValidityError> {
		if pre {
			block_flag::block();
		}
		Ok(Weight::zero())
	}
}
