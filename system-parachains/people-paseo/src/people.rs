// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;
use crate::xcm_config::LocationToAccountId;
use codec::{Decode, Encode, MaxEncodedLen};
use enumflags2::{bitflags, BitFlags};
use frame_support::{
	parameter_types, traits::ContainsPair, CloneNoBound, EqNoBound, PartialEqNoBound,
	RuntimeDebugNoBound,
};
use indiv_pallet_origin_restriction::Allowance;
use indiv_support::traits::Alias;
use pallet_identity::{Data, IdentityInformationProvider};
use parachains_common::{impls::ToParentTreasury, DAYS};
use scale_info::TypeInfo;
use sp_runtime::{
	traits::{AccountIdConversion, Verify},
	MultiSignature, RuntimeDebug,
};
use sp_statement_store::runtime_api::ValidStatement;
use verifiable::ring_vrf_impl::BandersnatchVrfVerifiable;
use xcm::latest::prelude::{BodyId, Location, Parachain, GeneralIndex};

parameter_types! {
	//   27 | Min encoded size of `Registration`
	// - 10 | Min encoded size of `IdentityInfo`
	// -----|
	//   17 | Min size without `IdentityInfo` (accounted for in byte deposit)
	pub const BasicDeposit: Balance = system_para_deposit(1, 17);
	pub const ByteDeposit: Balance = system_para_deposit(0, 1);
	pub const UsernameDeposit: Balance = system_para_deposit(0, 32);
	pub const SubAccountDeposit: Balance = system_para_deposit(1, 53);
	pub RelayTreasuryAccount: AccountId =
		parachains_common::TREASURY_PALLET_ID.into_account_truncating();
	pub const GeneralAdminBodyId: BodyId = BodyId::Administration;
	pub StableAssetLocation: Location = Location::new(
		1,
		[Parachain(2034), GeneralIndex(222)]
	);
}

pub type IdentityAdminOrigin = EitherOfDiverse<
	EnsureRoot<AccountId>,
	EitherOf<
		EnsureXcm<IsVoiceOfBody<RelayChainLocation, GeneralAdminBodyId>>,
		EnsureXcm<IsVoiceOfBody<AssetHubLocation, GeneralAdminBodyId>>,
	>,
>;

impl pallet_identity::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type BasicDeposit = BasicDeposit;
	type ByteDeposit = ByteDeposit;
	type UsernameDeposit = UsernameDeposit;
	type SubAccountDeposit = SubAccountDeposit;
	type MaxSubAccounts = ConstU32<100>;
	type IdentityInformation = IdentityInfo;
	type MaxRegistrars = ConstU32<20>;
	type Slashed = ToParentTreasury<RelayTreasuryAccount, LocationToAccountId, Runtime>;
	type ForceOrigin = EnsureRoot<Self::AccountId>;
	type RegistrarOrigin = IdentityAdminOrigin;
	type OffchainSignature = Signature;
	type SigningPublicKey = <Signature as Verify>::Signer;
	type UsernameAuthorityOrigin = IdentityAdminOrigin;
	type PendingUsernameExpiration = ConstU32<{ 7 * DAYS }>;
	type UsernameGracePeriod = ConstU32<{ 3 * DAYS }>;
	type MaxSuffixLength = ConstU32<7>;
	type MaxUsernameLength = ConstU32<32>;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ();
	type WeightInfo = weights::pallet_identity::WeightInfo<Runtime>;
}

/// The fields that we use to identify the owner of an account with. Each corresponds to a field
/// in the `IdentityInfo` struct.
#[bitflags]
#[repr(u64)]
#[derive(Clone, Copy, PartialEq, Eq, RuntimeDebug)]
pub enum IdentityField {
	Display,
	Legal,
	Web,
	Matrix,
	Email,
	PgpFingerprint,
	Image,
	Twitter,
	GitHub,
	Discord,
}

/// Information concerning the identity of the controller of an account.
#[derive(
	CloneNoBound,
	Encode,
	Decode,
	DecodeWithMemTracking,
	EqNoBound,
	MaxEncodedLen,
	PartialEqNoBound,
	RuntimeDebugNoBound,
	TypeInfo,
)]
#[codec(mel_bound())]
pub struct IdentityInfo {
	/// A reasonable display name for the controller of the account. This should be whatever the
	/// account is typically known as and should not be confusable with other entities, given
	/// reasonable context.
	///
	/// Stored as UTF-8.
	pub display: Data,

	/// The full legal name in the local jurisdiction of the entity. This might be a bit
	/// long-winded.
	///
	/// Stored as UTF-8.
	pub legal: Data,

	/// A representative website held by the controller of the account.
	///
	/// NOTE: `https://` is automatically prepended.
	///
	/// Stored as UTF-8.
	pub web: Data,

	/// The Matrix (e.g. for Element) handle held by the controller of the account. Previously,
	/// this was called `riot`.
	///
	/// Stored as UTF-8.
	pub matrix: Data,

	/// The email address of the controller of the account.
	///
	/// Stored as UTF-8.
	pub email: Data,

	/// The PGP/GPG public key of the controller of the account.
	pub pgp_fingerprint: Option<[u8; 20]>,

	/// A graphic image representing the controller of the account. Should be a company,
	/// organization or project logo or a headshot in the case of a human.
	pub image: Data,

	/// The Twitter identity. The leading `@` character may be elided.
	pub twitter: Data,

	/// The GitHub username of the controller of the account.
	pub github: Data,

	/// The Discord username of the controller of the account.
	pub discord: Data,
}

impl IdentityInformationProvider for IdentityInfo {
	type FieldsIdentifier = u64;

	fn has_identity(&self, fields: Self::FieldsIdentifier) -> bool {
		self.fields().bits() & fields == fields
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn create_identity_info() -> Self {
		let data = Data::Raw(vec![0; 32].try_into().unwrap());

		IdentityInfo {
			display: data.clone(),
			legal: data.clone(),
			web: data.clone(),
			matrix: data.clone(),
			email: data.clone(),
			pgp_fingerprint: Some([0; 20]),
			image: data.clone(),
			twitter: data.clone(),
			github: data.clone(),
			discord: data,
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn all_fields() -> Self::FieldsIdentifier {
		use enumflags2::BitFlag;
		IdentityField::all().bits()
	}
}

impl IdentityInfo {
	pub(crate) fn fields(&self) -> BitFlags<IdentityField> {
		let mut res = <BitFlags<IdentityField>>::empty();
		if !self.display.is_none() {
			res.insert(IdentityField::Display);
		}
		if !self.legal.is_none() {
			res.insert(IdentityField::Legal);
		}
		if !self.web.is_none() {
			res.insert(IdentityField::Web);
		}
		if !self.matrix.is_none() {
			res.insert(IdentityField::Matrix);
		}
		if !self.email.is_none() {
			res.insert(IdentityField::Email);
		}
		if self.pgp_fingerprint.is_some() {
			res.insert(IdentityField::PgpFingerprint);
		}
		if !self.image.is_none() {
			res.insert(IdentityField::Image);
		}
		if !self.twitter.is_none() {
			res.insert(IdentityField::Twitter);
		}
		if !self.github.is_none() {
			res.insert(IdentityField::GitHub);
		}
		if !self.discord.is_none() {
			res.insert(IdentityField::Discord);
		}
		res
	}
}

/// A `Default` identity. This is given to users who get a username but have not set an identity.
impl Default for IdentityInfo {
	fn default() -> Self {
		IdentityInfo {
			display: Data::None,
			legal: Data::None,
			web: Data::None,
			matrix: Data::None,
			email: Data::None,
			pgp_fingerprint: None,
			image: Data::None,
			twitter: Data::None,
			github: Data::None,
			discord: Data::None,
		}
	}
}

parameter_types! {
	pub const OriginRestrictionAllowanceMax: Balance = 10 * UNITS;
	pub const OriginRestrictionAllowanceRecovery: Balance =
		OriginRestrictionAllowanceMax::get() / (24 * 60 * 10); // fully recovers in one day.
}

#[derive(
	Encode,
	Decode,
	DecodeWithMemTracking,
	Clone,
	PartialEq,
	Eq,
	RuntimeDebug,
	MaxEncodedLen,
	scale_info::TypeInfo,
)]
pub enum RestrictedEntity {
	LitePerson(AccountId),
}

impl indiv_pallet_origin_restriction::RestrictedEntity<OriginCaller, Balance> for RestrictedEntity {
	fn allowance(&self) -> Allowance<Balance> {
		match self {
			RestrictedEntity::LitePerson(_) => Allowance {
				max: OriginRestrictionAllowanceMax::get(),
				recovery_per_block: OriginRestrictionAllowanceRecovery::get(),
			},
		}
	}

	fn restricted_entity(origin_caller: &OriginCaller) -> Option<Self> {
		use indiv_pallet_people_lite::Origin::*;
		use OriginCaller::*;
		match origin_caller {
			PeopleLite(LitePerson(account_id)) =>
				Some(RestrictedEntity::LitePerson(account_id.clone())),
			_ => None,
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn benchmarked_restricted_origin() -> OriginCaller {
		use sp_core::crypto::Pair as _;
		use sp_runtime::{traits::IdentifyAccount, MultiSigner};
		let pair = sp_core::sr25519::Pair::from_string("//Alice", None)
			.expect("static values are valid; qed");
		let signer = MultiSigner::Sr25519(pair.public());
		let account_id = signer.into_account();
		OriginCaller::PeopleLite(indiv_pallet_people_lite::Origin::LitePerson(account_id))
	}
}

pub struct OperationAllowedOneTimeExcess;
impl ContainsPair<RestrictedEntity, RuntimeCall> for OperationAllowedOneTimeExcess {
	fn contains(_entity: &RestrictedEntity, _call: &RuntimeCall) -> bool {
		false
	}
}

impl indiv_pallet_origin_restriction::Config for Runtime {
	type WeightInfo = weights::indiv_pallet_origin_restriction::WeightInfo<Runtime>;
	type RestrictedEntity = RestrictedEntity;
	type OperationAllowedOneTimeExcess = OperationAllowedOneTimeExcess;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct PeopleLiteBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl indiv_pallet_people_lite::BenchmarkHelper<AccountId, MultiSignature>
	for PeopleLiteBenchmarkHelper
{
	fn sign_message(message: &[u8]) -> (AccountId, MultiSignature) {
		use sp_core::Pair;
		use sp_runtime::traits::IdentifyAccount;
		let entropy = [1u8; 32];
		let pair = sp_core::ed25519::Pair::from_seed(&entropy);
		let account = pair.public().into_account().into();
		let secret = ed25519_zebra::SigningKey::from(entropy);
		let signature = sp_core::ed25519::Signature::from_raw(secret.sign(message).into());
		(account, signature.into())
	}
}

impl indiv_pallet_people_lite::Config for Runtime {
	type WeightInfo = weights::indiv_pallet_people_lite::WeightInfo<Runtime>;
	type AttestationAllowanceManager = EnsureRoot<AccountId>;
	type Crypto = BandersnatchVrfVerifiable;
	type AttestationSignature = MultiSignature;
	type LiteConsumerRegistrar = Resources;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = PeopleLiteBenchmarkHelper;
}

#[cfg(feature = "runtime-benchmarks")]
pub struct ResourcesBenchmarkHelper;
#[cfg(feature = "runtime-benchmarks")]
impl indiv_pallet_resources::benchmarking::BenchmarkHelper<Runtime> for ResourcesBenchmarkHelper {
	fn set_time(now: core::time::Duration) {
		// We don't call `set_timestamp` directly because it triggers checks such as aura slot
		pallet_timestamp::Now::<Runtime>::put(now.as_millis() as u64);
	}

	fn sign_message(message: &[u8]) -> (sp_runtime::AccountId32, MultiSignature) {
		use sp_core::Pair;
		use sp_runtime::traits::IdentifyAccount;
		let entropy = [1u8; 32];
		let pair = sp_core::ed25519::Pair::from_seed(&entropy);
		let account = pair.public().into_account().into();
		let secret = ed25519_zebra::SigningKey::from(entropy);
		let signature = sp_core::ed25519::Signature::from_raw(secret.sign(message).into());
		(account, signature.into())
	}
}

parameter_types! {
	pub const ResourcesMaxUsernameLength: u32 = 32;
	pub const ResourcesMinUsernameLength: u32 = 7;
	pub const ResourcesPersonAuthDuration: u32 = 90 * DAYS;
	pub const ResourcesMinPersonAuthUpdateInterval: u32 = 30 * DAYS;
	pub const ResourcesUsernameReservationDuration: u64 = 7 * DAYS as u64;
	pub const ResourcesLitePersonStatementLimit: ValidStatement = ValidStatement {
		max_count: 50,
		max_size: 5 * 1024,
	};
}

impl indiv_pallet_resources::Config for Runtime {
	type WeightInfo = weights::indiv_pallet_resources::WeightInfo<Runtime>;
	type Crypto = BandersnatchVrfVerifiable;
	type MaxUsernameLength = ResourcesMaxUsernameLength;
	type MinUsernameLength = ResourcesMinUsernameLength;
	type PersonAuthDuration = ResourcesPersonAuthDuration;
	type MinPersonAuthUpdateInterval = ResourcesMinPersonAuthUpdateInterval;
	type EnsurePerson = frame_system::EnsureNever<Alias>;
	type EnsureLitePerson = indiv_pallet_people_lite::EnsureLitePerson<Runtime>;
	type Clock = Timestamp;
	type OffchainSignature = MultiSignature;
	type UsernameReservationDuration = ResourcesUsernameReservationDuration;
	type LitePersonStatementLimit = ResourcesLitePersonStatementLimit;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = ResourcesBenchmarkHelper;
}
