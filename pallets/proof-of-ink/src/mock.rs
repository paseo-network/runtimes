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

use crate::*;
use codec::DecodeWithMemTracking;
use frame_support::{
	derive_impl,
	dispatch::GetDispatchInfo,
	ensure,
	pallet_prelude::{DispatchResultWithPostInfo, TransactionValidityError},
	parameter_types,
	storage::with_transaction,
	traits::{Consideration, TryMapSuccess},
	PalletId,
};
use frame_system::{pallet_prelude::BlockNumberFor, EnsureRoot, EnsureSigned};
use indiv_support::traits::{JudgementContext, Statement};
use sp_core::{ConstU16, ConstU32, ConstU64, H256};
use sp_runtime::{
	morph_types,
	testing::{TestSignature, UintAuthorityId},
	traits::{
		Applyable, BlakeTwo256, Checkable, DispatchInfoOf, IdentityLookup,
		TransactionExtension as TransactionExtensionTrait, ValidateResult,
	},
	transaction_validity::{InvalidTransaction, TransactionSource},
	BuildStorage, DispatchError, TransactionOutcome, Weight,
};

pub const DENIED_PAYMENT_ACCOUNT: u64 = 111;
#[derive(Clone, Eq, PartialEq, Encode, Decode, TypeInfo, Debug, Default, DecodeWithMemTracking)]
pub struct DenyPaymentFor111;
impl TransactionExtensionTrait<RuntimeCall> for DenyPaymentFor111 {
	const IDENTIFIER: &'static str = "DenyPaymentFor111";
	type Implicit = ();
	type Val = ();
	type Pre = ();
	fn weight(&self, _call: &RuntimeCall) -> Weight {
		Weight::zero()
	}
	fn validate(
		&self,
		origin: RuntimeOrigin,
		_call: &RuntimeCall,
		_info: &DispatchInfoOf<RuntimeCall>,
		_len: usize,
		_self_implicit: Self::Implicit,
		_inherited_implication: &impl Encode,
		_source: TransactionSource,
	) -> ValidateResult<Self::Val, RuntimeCall> {
		if let Some(111) = origin.as_signer() {
			Err(InvalidTransaction::Custom(3).into())
		} else {
			Ok((Default::default(), (), origin))
		}
	}

	fn prepare(
		self,
		_val: Self::Val,
		_origin: &RuntimeOrigin,
		_call: &RuntimeCall,
		_info: &DispatchInfoOf<RuntimeCall>,
		_len: usize,
	) -> Result<Self::Pre, TransactionValidityError> {
		Ok(())
	}
}

pub type AccountId = <Test as frame_system::Config>::AccountId;
pub type BlockNumber = u64;
pub type PoICall = crate::Call<Test>;

pub type TransactionExtension = (
	crate::extension::AsProofOfInkParticipant<Test>,
	frame_system::CheckNonce<Test>,
	DenyPaymentFor111,
);

pub type Header = sp_runtime::generic::Header<BlockNumber, sp_runtime::traits::BlakeTwo256>;
pub type Block = sp_runtime::generic::Block<Header, UncheckedExtrinsic>;
pub type UncheckedExtrinsic = sp_runtime::generic::UncheckedExtrinsic<
	u64,
	RuntimeCall,
	sp_runtime::testing::UintAuthorityId,
	TransactionExtension,
>;

pub struct MockOracle;

impl<C> StatementOracle<C> for MockOracle {
	type Ticket = [u8; 32];

	fn judge_statement(
		_: Statement,
		_: JudgementContext,
		_: Callback<(Self::Ticket, JudgementContext, Judgement), C>,
	) -> Result<Self::Ticket, DispatchError> {
		Ok(Self::Ticket::default())
	}
}

use verifiable::mock::Mock;

#[frame_support::pallet]
pub mod mock_people {
	use frame_support::pallet_prelude::*;
	use indiv_support::traits::{AddOnlyPeopleTrait, PersonalId};
	use sp_runtime::Saturating;

	#[pallet::config]
	pub trait Config: frame_system::Config {}

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::storage]
	pub type MockNextId<T> = StorageValue<_, PersonalId, ValueQuery>;

	#[pallet::storage]
	pub type MockReserved<T: Config> = StorageMap<_, Twox64Concat, PersonalId, (), OptionQuery>;

	#[pallet::storage]
	pub type MockRecognized<T: Config> = StorageMap<_, Twox64Concat, PersonalId, (), OptionQuery>;

	impl<T: Config> AddOnlyPeopleTrait for Pallet<T> {
		type Member = [u8; 32];
		fn reserve_new_id() -> PersonalId {
			let new_id = MockNextId::<T>::mutate(|id| {
				let new_id = *id;
				id.saturating_inc();
				new_id
			});
			MockReserved::<T>::insert(new_id, ());
			new_id
		}
		fn recognize_personhood(
			who: PersonalId,
			maybe_key: Option<Self::Member>,
		) -> Result<(), DispatchError> {
			// people pallets usually allow recognized people to be recognized again.
			// For simplicity this implementation doesn't, but we can modify when we need it.
			MockReserved::<T>::take(who).expect("We always reserve before recognizing.");
			assert!(!MockRecognized::<T>::contains_key(who), "Id already recognized");
			MockRecognized::<T>::insert(who, ());
			maybe_key.expect("We always recognize with key in the context of POI");
			Ok(())
		}
		fn cancel_id_reservation(personal_id: PersonalId) -> Result<(), DispatchError> {
			MockReserved::<T>::take(personal_id).expect("We only cancel reserved id");
			Ok(())
		}
		fn renew_id_reservation(personal_id: PersonalId) -> Result<(), DispatchError> {
			ensure!(
				MockNextId::<T>::get() > personal_id &&
					!MockRecognized::<T>::contains_key(personal_id) &&
					!MockReserved::<T>::contains_key(personal_id),
				DispatchError::Other("Invalid id reservation")
			);
			MockReserved::<T>::insert(personal_id, ());
			Ok(())
		}
		#[cfg(feature = "runtime-benchmarks")]
		type Secret = PersonalId;
		#[cfg(feature = "runtime-benchmarks")]
		fn mock_key(who: PersonalId) -> (Self::Member, Self::Secret) {
			let mut m = [0u8; 32];
			m[0..8].copy_from_slice(&who.to_le_bytes());
			(m, who)
		}
		#[cfg(feature = "runtime-benchmarks")]
		fn initialize_people_collection() {}
	}
}
use crate::extension::AsProofOfInkParticipantInfo;
pub use mock_people::{MockNextId, MockRecognized, MockReserved};

// Configure a mock runtime to test the pallet.
frame_support::construct_runtime!(
	pub enum Test
	{
		System: frame_system,
		Balances: pallet_balances,
		PoI: crate,
		MockPeople: mock_people,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type BaseCallFilter = frame_support::traits::Everything;
	type BlockWeights = ();
	type BlockLength = ();
	type DbWeight = ();
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
	type Nonce = u64;
	type Hash = H256;
	type Hashing = BlakeTwo256;
	type AccountId = u64;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Block = Block;
	type RuntimeEvent = RuntimeEvent;
	type BlockHashCount = ConstU64<250>;
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = pallet_balances::AccountData<u64>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
	type SS58Prefix = ConstU16<42>;
	type OnSetCode = ();
	type MaxConsumers = frame_support::traits::ConstU32<16>;
}

morph_types! {
	pub type Under10: TryMorph = |r: u64| -> Result<u64, ()> {
		if r < 10 { Ok(r) } else if r == u64::MAX { Err(()) } else { Ok(0) }
	};
}

parameter_types! {
	pub const PoiPotId: PalletId = PalletId(*b"poi/pot ");
	pub const ExistentialDeposit: u64 = 1;
}

pub type MaxActiveReferrals = ConstU32<10>;

#[cfg(feature = "runtime-benchmarks")]
impl BenchmarkHelper<Test> for Test {
	fn create_tickets(
		seed: u64,
	) -> sp_runtime::BoundedVec<ReferralTicket<u64>, MaxActiveReferrals> {
		sp_runtime::BoundedVec::try_from(vec![ReferralTicket { ticket: seed }]).unwrap()
	}

	fn create_ticket(seed: u64) -> (UintAuthorityId, u64) {
		(seed.into(), seed)
	}

	fn sign(seed: u64, msg: &[u8]) -> TestSignature {
		TestSignature(seed, msg.to_vec())
	}

	fn build_person_origin(personal_id: PersonalId) -> RuntimeOrigin {
		RuntimeOrigin::signed(personal_id)
	}

	fn setup_currency() {}
}

impl crate::Config for Test {
	type WeightInfo = ();
	type Oracle = MockOracle;
	type Deposit = ();
	type People = MockPeople;
	type Randomness = TestRandomness<Self>;
	type EnsurePerson = TryMapSuccess<EnsureSigned<u64>, Under10>;
	type TicketSignature = TestSignature;
	type TicketPublic = UintAuthorityId;
	type Ticket = u64;
	type DataStore = ();
	type MaxActiveReferrals = MaxActiveReferrals;
	type MaxRetryAttempts = ConstU32<1>;
	type MaxReimbursementValues = ConstU32<10>;
	type Currency = Balances;
	type PotId = PoiPotId;
	#[cfg(feature = "runtime-benchmarks")]
	type BenchmarkHelper = Test;
	type InvitationsOrigin = EnsureRoot<Self::AccountId>;
	type ManagerOrigin = EnsureRoot<Self::AccountId>;
	type Crypto = verifiable::mock::Mock;
}

impl pallet_balances::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeHoldReason = RuntimeHoldReason;
	type RuntimeFreezeReason = RuntimeFreezeReason;
	type WeightInfo = ();
	type Balance = u64;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type ReserveIdentifier = [u8; 8];
	type FreezeIdentifier = ();
	type MaxLocks = ConstU32<50>;
	type MaxReserves = ConstU32<50>;
	type MaxFreezes = ConstU32<50>;
	type DoneSlashHandler = ();
}

impl mock_people::Config for Test {}

/// Provides an implementation of [`frame_support::traits::Randomness`] that should only be used in
/// tests!
pub struct TestRandomness<T>(core::marker::PhantomData<T>);

impl<Output: codec::Decode + Default, T>
	frame_support::traits::Randomness<Output, BlockNumberFor<T>> for TestRandomness<T>
where
	T: frame_system::Config,
{
	fn random(subject: &[u8]) -> (Output, BlockNumberFor<T>) {
		use sp_runtime::traits::TrailingZeroInput;

		(
			Output::decode(&mut TrailingZeroInput::new(subject)).unwrap_or_default(),
			frame_system::Pallet::<T>::block_number(),
		)
	}
}

#[allow(dead_code)]
pub fn advance_to(b: BlockNumber) {
	while System::block_number() < b {
		System::set_block_number(System::block_number() + 1);
	}
}

pub fn advance_by(b: BlockNumber) {
	let initial_block = System::block_number();

	while System::block_number() < b + initial_block {
		System::set_block_number(System::block_number() + 1);
	}
}

pub fn mock_designs() -> Result<(), &'static str> {
	let families = [
		Family { kind: FamilyKind::Designed { count: 1000 }, id: [0u8; 32] },
		Family { kind: FamilyKind::Procedural { range: 10 }, id: [0u8; 32] },
		Family { kind: FamilyKind::ProceduralAccount, id: [0u8; 32] },
		Family { kind: FamilyKind::ProceduralPersonal, id: [0u8; 32] },
	];

	for (i, family) in families.iter().enumerate() {
		if !DesignFamilies::<Test>::contains_key(i as u16) {
			DesignFamilies::<Test>::insert(i as u16, family)
		};
	}

	for (i, family) in families.iter().enumerate() {
		ensure!(
			matches!(DesignFamilies::<Test>::get(i as u16), Some(actual_family) if actual_family == *family),
			"Design families not created."
		);
	}

	Ok(())
}

pub fn mock_candidate(
	id: AccountId,
	referrer: Option<AccountId>,
	commitment: Option<(InkChoice, Allocation)>,
	judging: Option<OracleTicketOf<Test>>,
	proven: bool,
) -> Result<Candidate<[u8; 32], (), u64, u64>, DispatchError> {
	let cred = match referrer {
		Some(referrer_id) => {
			People::<Test>::mutate_extant(referrer_id, |person| {
				person.active_referrals.try_push(id).unwrap();
				person.referrals += 1;
				if proven {
					person.pending_referral_rewards += 1;
					person.allowed_referral_tickets += 1;
				}
			});
			Credibility::Referred(referrer_id)
		},
		None => {
			<mock::Test as pallet::Config>::Deposit::new(
				&id,
				Footprint::from_mel::<(AccountId, CandidateOf<Test>)>(),
			)
			.unwrap();

			Credibility::Deposit(())
		},
	};
	let entropy = get_entropy(id);
	let entropy_since = System::block_number();

	let next_id = MockPeople::reserve_new_id();

	let status = match commitment {
		Some((design, allocation)) => {
			let design = PoI::bake_design(design, entropy, id, next_id)?;
			AllocationCount::<Test>::mutate(|n| n.saturating_inc());
			if proven {
				if let InkSpec::DesignedElective(family_id, design_index) = design {
					CommittedDesigns::<Test>::insert(
						family_id,
						design_index,
						DesignStatus::Committed,
					);
				}
				Candidate::Proven {
					design,
					reserved: next_id,
					was_referred: referrer.is_some(),
					was_invited: false,
				}
			} else {
				if let InkSpec::DesignedElective(family_id, design_index) = design {
					CommittedDesigns::<Test>::insert(
						family_id,
						design_index,
						DesignStatus::Reserved,
					);
				}
				Candidate::Selected {
					cred,
					entropy,
					since: entropy_since,
					reserved: next_id,
					design,
					allocation,
					judging,
					failed: 0,
				}
			}
		},
		None => Candidate::Applied { cred, entropy, entropy_since },
	};

	System::inc_sufficients(&id);
	Candidates::<Test>::insert(id, status);

	let candidate = Candidates::<Test>::get(id).unwrap();
	ensure!(
		matches!(
			candidate,
			Candidate::Applied { .. } | Candidate::Selected { .. } | Candidate::Proven { .. }
		),
		"Candidate not created."
	);

	ensure!(
		MockReserved::<Test>::contains_key(next_id),
		"Candidate was not reserved a personal ID."
	);

	Ok(candidate)
}

pub fn mock_person(id: AccountId, design: Option<InkSpec>) -> Result<(), &'static str> {
	MockNextId::<Test>::put(MockNextId::<Test>::get().max(id + 1));
	MockRecognized::<Test>::insert(id, ());
	let person = Person {
		design,
		active_referrals: Default::default(),
		allowed_referral_tickets: 1,
		pending_referral_rewards: Default::default(),
		bad_referrals: 0,
		successful_referrals: 0,
		referrals: 0,
		derivatives: 0,
		banned: false,
	};
	People::<Test>::insert(id, person);

	ensure!(
		matches!(
		People::<Test>::get(id).unwrap(), Person { active_referrals, bad_referrals, referrals, banned, allowed_referral_tickets, .. }
		if active_referrals.is_empty() && bad_referrals == 0 && referrals == 0 && !banned && allowed_referral_tickets == 1
		),
		"Person not created with correct defaults."
	);

	Ok(())
}

pub fn get_entropy(id: AccountId) -> [u8; 32] {
	(b"poi/apply", &id)
		.using_encoded(|s| {
			<<mock::Test as pallet::Config>::Randomness as Randomness<[u8; 32], BlockNumber>>::random(s).0
		})
}

pub fn mock_evidence() -> EvidenceHash {
	Default::default()
}

pub fn mock_key(
	id: AccountId,
) -> (<Mock as GenerateVerifiable>::Member, <Mock as GenerateVerifiable>::Secret) {
	let mut entropy = [0u8; 32];
	entropy[0..8].copy_from_slice(&id.to_le_bytes());
	let s = Mock::new_secret(entropy);
	let p = Mock::member_from_secret(&s);
	(p, s)
}

pub fn prepare_for_judgement(id: AccountId) -> (OracleTicketOf<Test>, JudgementContext) {
	let ticket: OracleTicketOf<Test> = [id as u8; 32];
	let context: JudgementContext = id.encode().try_into().unwrap();
	(ticket, context)
}

pub fn append_reimbursement_values(
	referred_value: u64,
	referrer_value: u64,
	count: u32,
) -> Result<(), &'static str> {
	let mut values = ReferredReimbursementValues::<Test>::get().unwrap_or_default();
	values
		.try_insert(0, (referred_value, count))
		.map_err(|_| "couldn't insert to referred values")?;
	ReferredReimbursementValues::<Test>::put(values);

	let mut values = ReferrerReimbursementValues::<Test>::get().unwrap_or_default();
	values
		.try_insert(0, (referrer_value, count))
		.map_err(|_| "couldn't insert to referrer values")?;
	ReferrerReimbursementValues::<Test>::put(values);
	Ok(())
}

pub fn new_config() -> ConfigRecord<BlockNumber> {
	ConfigRecord {
		reroll_timeout: 10 as BlockNumber, // one minute
		fasttrack_count: 1,
		maximum: 3,
		full_alloc_len: 64 * 1024 * 1024,
		full_alloc_count: 32,
		init_alloc_len: 2 * 1024 * 1024,
		init_alloc_count: 8,
		timeout: 10 as BlockNumber, // one minute
	}
}

// Map out happy path test cases to avoid code duplication
pub struct JudgedTest {
	// The judgement to input
	pub judgement: Judgement,
	// The allocation to input
	pub allocation: Allocation,
	// Run for referred candidate
	pub referred: bool,

	// Side effects should match a successful judgement
	pub should_succeed: bool,
	// If failure, is the candidate able to retry?
	pub soft_fail: bool,
	// If hard failure, is this a bannable reference?
	pub bannable: bool,
}

impl JudgedTest {
	pub fn new(
		judgement: Judgement,
		allocation: Allocation,
		referred: bool,
		should_succeed: bool,
		soft_fail: bool,
		bannable: bool,
	) -> Self {
		Self { judgement, allocation, referred, should_succeed, soft_fail, bannable }
	}
}

pub struct TestExt(ConfigRecord<u64>);
#[allow(dead_code)]
impl TestExt {
	pub fn new() -> Self {
		Self(new_config())
	}

	pub fn fasttrack_count(mut self, fasttrack_count: u32) -> Self {
		self.0.fasttrack_count = fasttrack_count;
		self
	}

	pub fn maximum(mut self, maximum: u32) -> Self {
		self.0.maximum = maximum;
		self
	}

	pub fn reroll_timeout(mut self, reroll_timeout: BlockNumber) -> Self {
		self.0.reroll_timeout = reroll_timeout;
		self
	}

	pub fn timeout(mut self, timeout: BlockNumber) -> Self {
		self.0.timeout = timeout;
		self
	}

	pub fn execute_with<R>(self, f: impl Fn() -> R) -> R {
		new_test_ext().execute_with(|| {
			Configuration::<Test>::put(self.0);
			f()
		})
	}
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	let mut c = frame_system::GenesisConfig::<Test>::default().build_storage().unwrap();
	pallet_balances::GenesisConfig::<Test> {
		balances: vec![(PoI::proof_of_ink_pot_id(), 1_000_000_000)],
		dev_accounts: None,
	}
	.assimilate_storage(&mut c)
	.unwrap();
	sp_io::TestExternalities::from(c)
}

pub fn exec_tx(
	who: u64,
	nonce: u64,
	call: impl Into<RuntimeCall>,
	participant_info: Option<AsProofOfInkParticipantInfo<Test>>,
) -> Result<DispatchResultWithPostInfo, TransactionValidityError> {
	let tx_ext = (
		crate::extension::AsProofOfInkParticipant::<Test>::new(participant_info),
		frame_system::CheckNonce::<Test>::from(nonce), // This nonce is irrelevant for now.
		DenyPaymentFor111,
	);

	let tx = UncheckedExtrinsic::new_signed(call.into(), who, UintAuthorityId(who), tx_ext);

	let info = tx.get_dispatch_info();
	let len = tx.encoded_size();

	let checked = Checkable::check(tx, &frame_system::ChainContext::<Test>::default())?;

	with_transaction(|| {
		let valid = checked.validate::<Test>(TransactionSource::External, &info, len);

		TransactionOutcome::Rollback(Result::<_, DispatchError>::Ok(valid))
	})
	.unwrap()?;

	checked.apply::<Test>(&info, len)
}

pub fn assert_reward_registered(rewards: Vec<AccountId>) {
	for reward in rewards {
		assert!(
			Balances::free_balance(reward) > 0,
			"expected reward transfer for account {reward}",
		);
	}
}

pub fn assert_reward_value(reward: AccountId, expected_value: u64) {
	assert_eq!(Balances::free_balance(reward), expected_value);
}
