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

//! # DotNS Gateway Pallet
//!
//! Gateway for verified persons (People and People Lite) to reserve and register usernames
//! in the dotNS smart contracts via pallet_revive.
//!
//! ## Overview
//!
//! This pallet:
//! - Accepts attester-based reservations for Lite People, verifying the attester's allowance and
//!   the user's signature.
//! - Accepts ring membership proofs for Full People username registration, verified against
//!   `members-subscriber` rings.
//! - Encodes EVM ABI calldata and dispatches contract calls to the `RootGatewayDispatcher` (which
//!   forwards to `DotnsPopController`) via a `ContractCaller` trait.
//!
//! ## Wiring
//!
//! The dispatcher contract checks whether the caller is root, so the runtime's `ContractCaller`
//! implementation must invoke the dispatcher as `RuntimeOrigin::root()`; non-Root calls
//! revert with `NotRoot`, surfaced here as
//! `Error::ContractRevert(DispatcherRevert::NotRoot)`.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
pub mod extension;
pub mod migration;
pub mod types;
pub mod weights;

pub use extension::{AsDotnsGateway, AsDotnsGatewayInfo};

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub use pallet::*;
pub use types::*;
pub use weights::WeightInfo;

use frame_support::traits::{OriginTrait, UnixTime};
use indiv_support::{
	context::{build_product_context, personhood},
	traits::{Alias, MembershipProver, RevisionIndex, RingExponent, RingIndex},
};
use sp_runtime::traits::{IdentifyAccount, Verify};
use verifiable::GenerateVerifiable;

#[frame_support::pallet]
pub mod pallet {

	use super::*;
	use alloc::vec::Vec;
	use frame_support::{dispatch::PostDispatchInfo, pallet_prelude::*};
	use frame_system::pallet_prelude::*;
	use sp_core::H160;

	/// Message prefix for candidate signature verification during name reservation.
	pub const RESERVE_MSG_PREFIX: &[u8] = b"pop:dotns-gateway:reserve";

	const LOG_TARGET: &str = "runtime::indiv-pallet-dotns-gateway";

	/// The in-code storage version. Bump it and add a migration when the layout of a
	/// storage item changes; see [`migration`].
	const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

	#[pallet::pallet]
	#[pallet::storage_version(STORAGE_VERSION)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config:
		frame_system::Config<
		RuntimeOrigin: From<Origin>
		                   + From<<Self::RuntimeOrigin as OriginTrait>::PalletsOrigin>
		                   + OriginTrait<
			PalletsOrigin: From<Origin>
			                   + TryInto<
				Origin,
				Error = <Self::RuntimeOrigin as OriginTrait>::PalletsOrigin,
			>,
		>,
		RuntimeCall: frame_support::traits::IsSubType<Call<Self>>,
	>
	{
		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;

		/// Runtime-wide network suffix used to derive product contexts.
		type Suffix: Get<indiv_support::context::ProductContextNetworkSuffix>;

		/// Ring-membership prover used to verify proofs sent to [`Pallet::register_name`].
		type MemberService: MembershipProver<
			Crypto: GenerateVerifiable<
				Config: TryFrom<RingExponent>,
				Proof: Send + Sync,
				Signature: Send + Sync,
			>,
		>;

		/// Contract call abstraction (backed by pallet_revive in production).
		type ContractCaller: ContractCaller;

		/// Converts AccountId to EVM H160 address.
		///
		/// In production, wired to pallet_revive's `AccountId32Mapper`.
		type AddressMapper: AddressMapper<Self::AccountId>;

		/// Maximum weight budget for a single contract call. The extrinsic pre-charges
		/// pallet overhead + this budget and refunds unused weight based on the actual
		/// weight reported by the contract caller.
		#[pallet::constant]
		type MaxContractCallWeight: Get<Weight>;

		/// Signature validity window, in seconds.
		/// A signature is accepted only if `now <= signed_at + MaxValiditySeconds`.
		#[pallet::constant]
		type MaxValiditySeconds: Get<u64>;

		/// Clock-skew tolerance for `signed_at`, in seconds.
		/// A signature is accepted only if `signed_at <= now + MaxFutureSkewSeconds`.
		#[pallet::constant]
		type MaxFutureSkewSeconds: Get<u64>;

		/// Unix time source.
		type UnixTime: UnixTime;

		/// The origin that can manage attestation allowances.
		type AttestationAllowanceManager: EnsureOrigin<Self::RuntimeOrigin>;

		/// The origin that can set the `RootGatewayDispatcher` contract address.
		type DispatcherAddressManager: EnsureOrigin<Self::RuntimeOrigin>;

		/// Signature type used by candidates to consent to name reservations.
		type AttestationSignature: Verify<Signer: IdentifyAccount<AccountId = Self::AccountId>>
			+ Parameter
			+ Send
			+ Sync;

		/// Benchmark helper.
		#[cfg(feature = "runtime-benchmarks")]
		type BenchmarkHelper: benchmarking::BenchmarkHelper<Self>;
	}

	// ========== Storage Items ==========

	/// Tracks which aliases have registered names via this gateway.
	#[pallet::storage]
	pub type AliasRegistration<T: Config> =
		StorageMap<_, Blake2_128Concat, Alias, RegistrationRecord<T>, OptionQuery>;

	/// Reverse lookup from a registering account to the alias it used.
	///
	/// Populated alongside [`AliasRegistration`] in [`Pallet::register_name`].
	#[pallet::storage]
	pub type AccountAlias<T: Config> =
		StorageMap<_, Blake2_128Concat, T::AccountId, Alias, OptionQuery>;

	/// Number of username reservations an attester is allowed to grant.
	#[pallet::storage]
	pub type AttestationAllowance<T: Config> =
		StorageMap<_, Blake2_128Concat, T::AccountId, u32, ValueQuery>;

	/// Records which account owns each reserved lite-person label, so
	/// [`Pallet::register_name`] can verify that a `Link::LiteUsername` target
	/// belongs to the caller.
	#[pallet::storage]
	pub type LiteLabelOwner<T: Config> =
		StorageMap<_, Blake2_128Concat, BaseLabel, T::AccountId, OptionQuery>;

	/// The dotNS labels each account acquired through this gateway.
	///
	/// Keyed by account so clients can watch a set of accounts with one storage
	/// subscription.
	///
	/// Warning: this map is not the source of truth; the dotNS contracts are. It
	/// is written on [`Pallet::reserve_name`] and [`Pallet::register_name`] only,
	/// so a label that changes purely contract-side (for example a transfer) is
	/// not reflected here and clients must re-verify on read via contract views.
	/// Names registered outside these paths never appear here at all, including
	/// pre-launch whitelist allocations, which claimants register directly against
	/// the dotNS registrar controller.
	/// The map is temporary and goes away once apps can observe contract storage
	/// directly: <https://github.com/paritytech/individuality-community/issues/52>.
	#[pallet::storage]
	pub type AccountNames<T: Config> =
		StorageMap<_, Blake2_128Concat, T::AccountId, AccountNameRecord, OptionQuery>;

	/// Address of the `RootGatewayDispatcher` contract. Must be set (via genesis or
	/// [`Pallet::set_dispatcher_address`]) before [`Pallet::reserve_name`] or
	/// [`Pallet::register_name`] can succeed.
	#[pallet::storage]
	pub type DispatcherAddress<T: Config> = StorageValue<_, H160, OptionQuery>;

	// ========== Origin ==========

	/// Origin for person calls in this pallet.
	#[pallet::origin]
	#[derive(
		Clone, PartialEq, Eq, Debug, Encode, Decode, MaxEncodedLen, TypeInfo, DecodeWithMemTracking,
	)]
	pub enum Origin {
		/// A full-person registration that has been authenticated by [`AsDotnsGateway`].
		PersonRegistration(Alias),
	}

	// ========== Events ==========

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// A username has been reserved for a candidate by an attester.
		NameReserved {
			/// The candidate whose name was reserved.
			candidate: T::AccountId,
			/// The attester who performed the reservation.
			attester: T::AccountId,
			/// The lite-person label (`<dns-stem>.<digits>`).
			lite_label: BaseLabel,
			/// The ECDH chat key stored alongside the lite-person label.
			chat_key: ChatKey,
			/// The optional base label reserved for future full-person claiming.
			reserved_base_label: Option<BaseLabel>,
		},
		/// A username has been registered by a verified person.
		NameRegistered {
			/// The alias derived from the ring proof.
			alias: Alias,
			/// The account that initiated the registration.
			account: T::AccountId,
			/// The registered full-person label.
			label: BaseLabel,
			/// How the full-person label relates to a lite-person label.
			link: Link,
		},
		/// Attestation allowance was increased for an account.
		AttestationAllowanceIncreased {
			/// The attester account.
			account: T::AccountId,
			/// The number of attestations added.
			count: u32,
		},
		/// All attestation allowance has been removed for the attester.
		AllAttestationAllowanceCleared {
			/// The attester account.
			attester: T::AccountId,
		},
		/// The `RootGatewayDispatcher` contract address was set.
		DispatcherAddressSet {
			/// The new contract address.
			address: H160,
		},
	}

	// ========== Errors ==========

	#[pallet::error]
	pub enum Error<T> {
		/// The alias has already registered a name.
		AlreadyRegistered,
		/// The contract call failed (non-revert: out-of-gas, low-level, or
		/// undecodable revert).
		ContractCallFailed,
		/// Contract reverted with a typed error from `RootGatewayDispatcher` or
		/// the `DotnsPopController` it forwards to.
		ContractRevert(DispatcherRevert),
		/// The name is invalid (empty or too long, or does not match the
		/// required DNS/lite-label format).
		InvalidName,
		/// The attester has no attestation allowance remaining.
		NoAttestationAllowance,
		/// The candidate's signature is invalid.
		InvalidAttestationSignature,
		/// The reservation signature is older than [`Config::MaxValiditySeconds`].
		ReservationSignatureExpired,
		/// The reservation signature claims to have been created more than
		/// [`Config::MaxValiditySeconds`] ahead of chain time.
		ReservationSignatureFromFuture,
		/// The `Link::LiteUsername` target is not owned by the caller (or has
		/// no on-chain ownership record).
		NotLiteLabelOwner,
		/// The `RootGatewayDispatcher` contract address has not been set.
		DispatcherAddressNotSet,
	}

	// ========== Extrinsics ==========

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Reserves a lite-person label on `DotnsPopController` (via `RootGatewayDispatcher`)
		/// for a Lite Person, and optionally enqueues a reservation for a base label the user
		/// intends to claim as a Full Person later.
		///
		/// The caller must be an attester with available allowance. The user's
		/// signature proves their consent to the reservation with this specific label
		/// and attester.
		///
		/// # Parameters
		/// - `candidate`: the Lite Person's account for whom the label is being reserved. Also the
		///   signer of `candidate_signature`.
		/// - `candidate_signature`: signature over the bytes returned by
		///   `Pallet::construct_reservation_message` — a SCALE-encoded tuple binding the attester,
		///   candidate, the candidate's chosen *base* username, and chat key. The digit suffix of
		///   `lite_label` is **not** part of the signed message (the digits are allocated
		///   server-side after the candidate signs, so the candidate cannot commit to them at
		///   signing time).
		/// - `lite_label`: the lite-person label. Must be `<dns-stem>.<2+ digits>` (e.g.
		///   `alice.42`), matching `StringUtils.isLitePersonLabel` in the contracts. Only the
		///   `<dns-stem>` portion enters the signed message; the digits ride along as an unsigned
		///   extrinsic argument used for the contract calldata and storage key.
		/// - `chat_key`: 65-byte public key used for ECDH chat.
		/// - `reserved_base_label`: optional base label reserved for the candidate to later claim
		///   as a Full Person. Must be a single DNS label (e.g. `alice`).
		/// - `signed_at`: Unix-time second at which the candidate signed the reservation message.
		///   The signature is accepted only if `signed_at <= now + MaxFutureSkewSeconds` and `now
		///   <= signed_at + MaxValiditySeconds`, where `now` comes from [`Config::UnixTime`].
		#[pallet::call_index(0)]
		#[pallet::weight(
			<T as Config>::WeightInfo::reserve_name()
				.saturating_add(T::MaxContractCallWeight::get())
		)]
		pub fn reserve_name(
			origin: OriginFor<T>,
			candidate: T::AccountId,
			candidate_signature: T::AttestationSignature,
			lite_label: BaseLabel,
			chat_key: ChatKey,
			reserved_base_label: Option<BaseLabel>,
			signed_at: u64,
		) -> DispatchResultWithPostInfo {
			let attester = ensure_signed(origin)?;

			ensure!(lite_label.is_valid_lite(), Error::<T>::InvalidName);
			if let Some(ref label) = reserved_base_label {
				ensure!(label.is_valid_person(), Error::<T>::InvalidName);
			}

			let now = <T as Config>::UnixTime::now().as_secs();
			ensure!(
				signed_at <= now.saturating_add(T::MaxFutureSkewSeconds::get()),
				Error::<T>::ReservationSignatureFromFuture,
			);
			ensure!(
				now <= signed_at.saturating_add(T::MaxValiditySeconds::get()),
				Error::<T>::ReservationSignatureExpired,
			);

			// Verifying the candidate's signature.
			let msg = Self::construct_reservation_message(
				&candidate,
				&attester,
				lite_label.lite_base(),
				chat_key.as_bytes(),
				reserved_base_label.as_ref().map(BaseLabel::as_slice),
				signed_at,
			);
			ensure!(
				candidate_signature.verify(&msg[..], &candidate),
				Error::<T>::InvalidAttestationSignature,
			);

			// Checking and decrementing attestation allowance
			let available = AttestationAllowance::<T>::get(&attester)
				.checked_sub(1)
				.ok_or(Error::<T>::NoAttestationAllowance)?;

			if available > 0 {
				AttestationAllowance::<T>::insert(&attester, available);
			} else {
				AttestationAllowance::<T>::remove(&attester);
			}

			// Encoding and calling the contract
			let candidate_h160 = T::AddressMapper::to_address(&candidate);
			let calldata = match reserved_base_label.as_ref() {
				Some(base) => reserveBaseNameCall::new_abi_encoded(
					lite_label.as_slice(),
					candidate_h160,
					chat_key.as_slice(),
					base.as_slice(),
				),
				None => reserveLiteNameCall::new_abi_encoded(
					lite_label.as_slice(),
					candidate_h160,
					chat_key.as_slice(),
				),
			};
			let contract_weight = Self::call_dispatcher(calldata)?;

			LiteLabelOwner::<T>::insert(&lite_label, &candidate);
			AccountNames::<T>::mutate(&candidate, |record| {
				record.get_or_insert_with(AccountNameRecord::default).lite =
					Some(NameEntry { label: lite_label.clone(), chat: Some(chat_key) });
			});

			Self::deposit_event(Event::NameReserved {
				candidate,
				attester,
				lite_label,
				chat_key,
				reserved_base_label,
			});
			Ok(PostDispatchInfo {
				actual_weight: Some(
					<T as Config>::WeightInfo::reserve_name().saturating_add(contract_weight),
				),
				pays_fee: Pays::No,
			})
		}

		/// Registers a full-person label on `DotnsPopController` (via `RootGatewayDispatcher`)
		/// for a fully proven person.
		///
		/// The origin must be [`Origin::PersonRegistration`], produced by [`AsDotnsGateway`] after
		/// it verifies the ring membership proof and the off-chain signature from `who`. All
		/// authentication, format and replay checks live in the extension.
		///
		/// # Parameters
		/// - `who`: the account that owns the full-person label; its EVM-mapped address is the
		///   recipient on the `DotnsPopController` contract. Must match the `who` already
		///   authenticated by [`AsDotnsGateway`].
		/// - `label`: the full-person label. Must be a single DNS label (e.g. `alice`), matching
		///   `StringUtils.isSingleLabel` in the contracts.
		/// - `link`: how the full-person label relates to a pre-existing lite-person label:
		///   - `Link::LiteUsername(lite_label)` — links to an existing lite-person label (must
		///     match the lite format `<dns-stem>.<2+ digits>`); the chat key is inherited from the
		///     lite entry.
		///   - `Link::None(chat_key)` — standalone registration with a fresh ECDH chat key.
		#[pallet::call_index(1)]
		#[pallet::weight(
			<T as Config>::WeightInfo::register_name()
				.saturating_add(T::MaxContractCallWeight::get())
		)]
		pub fn register_name(
			origin: OriginFor<T>,
			who: T::AccountId,
			label: BaseLabel,
			link: Link,
		) -> DispatchResultWithPostInfo {
			let alias = Self::ensure_person_registration(origin)?;

			let who_h160 = T::AddressMapper::to_address(&who);
			let calldata = registerBaseNameCall::new_abi_encoded(label.as_slice(), who_h160, &link);
			let contract_weight = Self::call_dispatcher(calldata)?;

			AliasRegistration::<T>::insert(
				alias,
				RegistrationRecord { collection: Collection::People, account: who.clone() },
			);
			AccountAlias::<T>::insert(&who, alias);
			AccountNames::<T>::mutate(&who, |record| {
				let record = record.get_or_insert_with(AccountNameRecord::default);
				let chat = match &link {
					Link::None(chat_key) => Some(*chat_key),
					// The contracts copy the lite label's key to the full label. The pallet
					// holds that key only when the linked label is the recorded one.
					Link::LiteUsername(lite) => record
						.lite
						.as_ref()
						.filter(|entry| entry.label == *lite)
						.and_then(|entry| entry.chat),
				};
				record.full = Some(NameEntry { label: label.clone(), chat });
			});

			Self::deposit_event(Event::NameRegistered { alias, account: who, label, link });
			Ok(PostDispatchInfo {
				actual_weight: Some(
					<T as Config>::WeightInfo::register_name().saturating_add(contract_weight),
				),
				pays_fee: Pays::No,
			})
		}

		/// Grants attestation allowance to an attester account.
		///
		/// The origin must be `AttestationAllowanceManager`.
		#[pallet::call_index(2)]
		#[pallet::weight(<T as Config>::WeightInfo::increase_attestation_allowance())]
		pub fn increase_attestation_allowance(
			origin: OriginFor<T>,
			account: T::AccountId,
			count: u32,
		) -> DispatchResult {
			T::AttestationAllowanceManager::ensure_origin(origin)?;
			AttestationAllowance::<T>::mutate(&account, |old| {
				*old = old.saturating_add(count);
			});
			Self::deposit_event(Event::AttestationAllowanceIncreased { account, count });
			Ok(())
		}

		/// Clears all attestation allowance for an attester account.
		///
		/// The origin must be `AttestationAllowanceManager`.
		#[pallet::call_index(3)]
		#[pallet::weight(<T as Config>::WeightInfo::clear_attestation_allowance())]
		pub fn clear_attestation_allowance(
			origin: OriginFor<T>,
			account: T::AccountId,
		) -> DispatchResult {
			T::AttestationAllowanceManager::ensure_origin(origin)?;
			AttestationAllowance::<T>::remove(&account);
			Self::deposit_event(Event::AllAttestationAllowanceCleared { attester: account });
			Ok(())
		}

		/// Sets the `RootGatewayDispatcher` contract address.
		///
		/// The origin must be `DispatcherAddressManager`. Overwrites any prior value.
		#[pallet::call_index(4)]
		#[pallet::weight(<T as Config>::WeightInfo::set_dispatcher_address())]
		pub fn set_dispatcher_address(origin: OriginFor<T>, address: H160) -> DispatchResult {
			T::DispatcherAddressManager::ensure_origin(origin)?;
			DispatcherAddress::<T>::put(address);
			Self::deposit_event(Event::DispatcherAddressSet { address });
			Ok(())
		}
	}

	// ========== Internal Functions ==========

	impl<T: Config> Pallet<T> {
		/// Builds the payload the candidate signs to authorize a
		/// name reservation.
		pub(crate) fn construct_reservation_message(
			candidate: &T::AccountId,
			attester: &T::AccountId,
			username_base: &[u8],
			chat_key: &[u8; 65],
			reserved_base_label: Option<&[u8]>,
			signed_at: u64,
		) -> Vec<u8> {
			(
				RESERVE_MSG_PREFIX,
				candidate,
				attester,
				username_base,
				chat_key.as_slice(),
				reserved_base_label,
				signed_at,
			)
				.encode()
		}

		/// Hashes the registration intent (`who`, `label`, `link`) into a
		/// 32-byte digest.
		pub(crate) fn construct_register_proof_message(
			who: &T::AccountId,
			label: &[u8],
			link: &Link,
		) -> [u8; 32] {
			(who, label, link).using_encoded(sp_io::hashing::blake2_256)
		}

		/// Calls `RootGatewayDispatcher` with the given ABI-encoded `calldata`
		/// (which the dispatcher forwards to `DotnsPopController`) and returns
		/// the weight reported by the contract caller.
		fn call_dispatcher(calldata: Vec<u8>) -> Result<Weight, Error<T>> {
			let address =
				DispatcherAddress::<T>::get().ok_or(Error::<T>::DispatcherAddressNotSet)?;
			T::ContractCaller::call(address, calldata, 0)
				.map(|(_, weight)| weight)
				.map_err(Error::<T>::from)
		}

		/// Verifies a ring membership proof against the `MemberService`.
		pub(crate) fn verify_proof(
			collection: &Collection,
			ring_index: RingIndex,
			revision: RevisionIndex,
			proof: &ProofOf<T>,
			message: &[u8],
		) -> Result<Alias, DispatchError> {
			let identifier = collection.identifier();
			let context = Self::proof_context();
			let validated = T::MemberService::verify_membership(
				identifier, proof, ring_index, revision, context, message,
			)?;
			Ok(validated.alias)
		}

		pub fn proof_context() -> indiv_support::traits::Context {
			build_product_context(
				personhood::PRODUCT_NAME,
				&T::Suffix::get(),
				personhood::DOTNS_GATEWAY,
			)
		}

		/// Extracts the alias carried by [`Origin::PersonRegistration`], fails for any other
		/// origin.
		pub(crate) fn ensure_person_registration(
			origin: OriginFor<T>,
		) -> Result<Alias, DispatchError> {
			match origin.into_caller().try_into() {
				Ok(Origin::PersonRegistration(alias)) => Ok(alias),
				_ => Err(DispatchError::BadOrigin),
			}
		}
	}

	impl<T: Config> From<ContractCallError> for Error<T> {
		fn from(err: ContractCallError) -> Self {
			let Some(bytes) = err.revert_data.as_deref() else {
				return Error::<T>::ContractCallFailed;
			};
			match decode_revert(bytes) {
				Some(variant) => Error::<T>::ContractRevert(variant),
				None => {
					log::warn!(
						target: LOG_TARGET,
						"unknown contract revert selector; data = {bytes:?}",
					);
					Error::<T>::ContractCallFailed
				},
			}
		}
	}

	// ========== Genesis ==========

	#[pallet::genesis_config]
	#[derive(frame_support::DefaultNoBound)]
	pub struct GenesisConfig<T: Config> {
		/// Initial `RootGatewayDispatcher` contract address. When `None`, the
		/// address must be set via [`Pallet::set_dispatcher_address`] before
		/// [`Pallet::reserve_name`] or [`Pallet::register_name`] can succeed.
		pub dispatcher_address: Option<H160>,
		#[serde(skip)]
		pub _phantom: PhantomData<T>,
	}

	#[pallet::genesis_build]
	impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
		fn build(&self) {
			if let Some(address) = self.dispatcher_address {
				DispatcherAddress::<T>::put(address);
			}
		}
	}
}
