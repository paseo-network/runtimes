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

//! Proof-of-Personhood system.
//!
//! Warning: This pallet must be configured with some spam prevention mechanism.
//! The spam prevention mechanism must limit how many calls the origin
//! `ReferredCandidate` can do.
//! The recommended approach is to use pallet-origins-restriction.

#![cfg_attr(not(feature = "std"), no_std)]
#![recursion_limit = "128"]

use codec::{Decode, Encode, MaxEncodedLen};
use frame_support::{
	dispatch::PostDispatchInfo,
	pallet_prelude::DispatchResult,
	traits::{
		fungible::{Inspect, Mutate},
		tokens::Preservation,
		EnsureOrigin, Footprint, IsSubType, OriginTrait, Randomness,
	},
	DefaultNoBound,
};
use indiv_support::traits::{
	AddOnlyPeopleTrait, Callback, EvidenceHash, InkSpec, Judgement, JudgementContext, PersonalId,
	Statement::ProofOfInk, StatementOracle, Truth::*,
};
use scale_info::TypeInfo;
use sp_arithmetic::traits::Saturating;
use sp_runtime::traits::{BadOrigin, IdentifyAccount, Verify};
use sp_std::vec;
use verifiable::GenerateVerifiable;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
pub mod extension;
pub mod runtime_api;
pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
pub use benchmarking::BenchmarkHelper;
pub use pallet::*;
pub use weights::WeightInfo;

pub use indiv_support::traits::AllocateStorage;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::{
		pallet_prelude::{DispatchResultWithPostInfo, ValueQuery, *},
		traits::{Consideration, Defensive},
		PalletId, Twox64Concat,
	};
	use frame_system::pallet_prelude::*;
	use sp_runtime::traits::AccountIdConversion;

	/// The prefix used when proving the ownership of keys.
	pub const PROOF_OF_OWNERSHIP_PREFIX: &[u8; 18] = b"pop register using";

	const LOG_TARGET: &str = "runtime::indiv-pallet-proof-of-ink";

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	/// The configuration of the pallet proof-of-ink.
	///
	/// Warning: This pallet must be configured with some spam prevention mechanism.
	/// The spam prevention mechanism must limit how many calls the origin
	/// `ReferredCandidate` can do.
	/// The recommended approach is to use pallet-origins-restriction.
	#[pallet::config]
	pub trait Config:
		frame_system::Config<
		RuntimeOrigin: From<Origin<Self>>
		                   + Into<Result<Origin<Self>, Self::RuntimeOrigin>>
		                   + OriginTrait<PalletsOrigin: From<Origin<Self>>>,
		RuntimeCall: IsSubType<Call<Self>>,
	>
	{
		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;

		/// Our ticket to allow some storage.
		type Deposit: Consideration<Self::AccountId, Footprint>;

		/// Who to tell when we recognise personhood.
		type People: AddOnlyPeopleTrait;

		/// How to recognise an origin representing a person.
		type EnsurePerson: EnsureOrigin<OriginFor<Self>, Success = PersonalId>;

		/// Signature used to verify tickets.
		///
		/// Can verify whether an `Self::TicketPublic` created a signature.
		type TicketSignature: Verify<Signer = Self::TicketPublic> + Parameter;

		/// Public key used to derive the ticket.
		type TicketPublic: IdentifyAccount<AccountId = Self::Ticket>;

		/// Ticket type used to refer candidates.
		type Ticket: Parameter + MaxEncodedLen;

		/// Oracle used to determine whether the evidence supplied of getting a tattoo is real
		/// or not.
		type Oracle: StatementOracle<Self::RuntimeCall>;

		/// Randomness beacon when offering a procedurally seeded PoI.
		type Randomness: Randomness<Entropy, BlockNumberFor<Self>>;

		/// The storage allocator.
		type DataStore: AllocateStorage<Self::AccountId>;

		/// The maximum number of active referrals a user can have open.
		/// It cannot exceed 200.
		#[pallet::constant]
		type MaxActiveReferrals: Get<u32>;

		/// The maximum number of retries a user can have after a failed case.
		type MaxRetryAttempts: Get<u32>;

		/// The maximum number of reimbursement values.
		type MaxReimbursementValues: Get<u32>;

		/// Currency used for reward payouts.
		type Currency: Inspect<Self::AccountId> + Mutate<Self::AccountId>;

		/// Account Identifier from which the reward pot is generated.
		///
		/// The account must be funded so rewards can be paid during registration and
		/// referral reward claims.
		///
		/// The `PalletId` must be a constant as it is exposed in the metadata.
		/// Changing this value with a runtime upgrade can be done at any time, the new account will
		/// then need to be funded accordingly.
		#[pallet::constant]
		type PotId: Get<PalletId>;

		/// The origin that can issue proof-of-ink invitations.
		type InvitationsOrigin: EnsureOrigin<Self::RuntimeOrigin>;

		/// The origin allowed to perform privileged management operations on this pallet.
		type ManagerOrigin: EnsureOrigin<Self::RuntimeOrigin>;

		/// Trait allowing cryptographic operation for the member key.
		type Crypto: GenerateVerifiable<Member = MemberOf<Self>>;

		/// Helper type for benchmarks.
		#[cfg(feature = "runtime-benchmarks")]
		type BenchmarkHelper: BenchmarkHelper<Self>;
	}

	pub type OracleTicketOf<T> = <<T as Config>::Oracle as StatementOracle<
		<T as frame_system::Config>::RuntimeCall,
	>>::Ticket;

	pub type CandidateOf<T> =
		Candidate<OracleTicketOf<T>, <T as Config>::Deposit, BlockNumberFor<T>, TicketOf<T>>;

	pub type MemberOf<T> = <<T as Config>::People as AddOnlyPeopleTrait>::Member;
	pub type BalanceOf<T> =
		<<T as Config>::Currency as Inspect<<T as frame_system::Config>::AccountId>>::Balance;

	/// Represents means of invitation, assignable to an account.
	type TicketOf<T> =
		<<<T as Config>::TicketSignature as Verify>::Signer as IdentifyAccount>::AccountId;

	// Not yet available, but planned.
	//	pub type Shield = [u8; 32];
	//	pub type ShieldCommit = [u8; 32];

	pub type Entropy = [u8; 32];
	pub type ProceduralSeed = [u8; 4];
	pub type FamilyIndex = u16;
	pub type DesignIndex = u16;
	pub type VariantIndex = u8;
	pub type Counter = u32;
	pub type FamilyId = [u8; 32];

	#[derive(
		Clone, PartialEq, Eq, Debug, Encode, Decode, MaxEncodedLen, TypeInfo, DecodeWithMemTracking,
	)]
	pub struct ConfigRecord<BlockNumber> {
		pub reroll_timeout: BlockNumber,
		pub fasttrack_count: u32,
		pub maximum: u32,
		pub full_alloc_len: u64,
		pub full_alloc_count: u32,
		pub init_alloc_len: u64,
		pub init_alloc_count: u32,
		pub timeout: BlockNumber,
	}

	impl<BlockNumber: From<u32>> Default for ConfigRecord<BlockNumber> {
		fn default() -> Self {
			Self {
				// 7 days in blocks (assuming 2 seconds block time: 30 blocks per minute)
				reroll_timeout: (7 * 24 * 60 * 30).into(),
				fasttrack_count: 4,
				maximum: 1000,
				full_alloc_len: 64 * 1024 * 1024,
				full_alloc_count: 32,
				init_alloc_len: 2 * 1024 * 1024,
				init_alloc_count: 8,
				timeout: (7 * 24 * 60 * 30).into(),
			}
		}
	}

	#[derive(
		Clone, PartialEq, Eq, Debug, Encode, Decode, MaxEncodedLen, TypeInfo, DecodeWithMemTracking,
	)]
	pub enum FamilyKind {
		Designed { count: DesignIndex },
		ProceduralAccount,
		ProceduralPersonal,
		Procedural { range: VariantIndex },
	}

	/// Structure describing a design family.
	#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, MaxEncodedLen, TypeInfo)]
	pub struct Family {
		/// The type of design this family offers.
		pub kind: FamilyKind,
		/// A unique, on-chain and ideally reproducible identifier for this family.
		pub id: FamilyId,
	}

	#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, MaxEncodedLen, TypeInfo)]
	pub enum DesignStatus {
		Reserved,
		Committed,
	}

	#[derive(
		Clone, PartialEq, Eq, Debug, Encode, Decode, MaxEncodedLen, TypeInfo, DecodeWithMemTracking,
	)]
	pub enum InkChoice {
		DesignedElective(FamilyIndex, DesignIndex),
		ProceduralAccount(FamilyIndex),
		ProceduralPersonal(FamilyIndex),
		Procedural(FamilyIndex, VariantIndex),
		ProceduralDerivative(PersonalId, Option<PersonalId>),
		// Not yet available, but planned.
		//		ProceduralShielded(FamilyIndex, Entropy, VariantIndex, ShieldCommit),
		//		ProceduralShieldedDerivative(PersonalId, Option<PersonalId>),
	}

	#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, MaxEncodedLen, TypeInfo)]
	pub enum Credibility<Deposit, PersonalId, AccountId> {
		Referred(PersonalId),
		Deposit(Deposit),
		Invited(AccountId),
	}

	#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, MaxEncodedLen, TypeInfo)]
	pub enum Allocation {
		Initial,
		InitDone,
		Full,
	}

	#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, MaxEncodedLen, TypeInfo)]
	pub enum ReferralReward<AccountId> {
		Destination(AccountId),
	}

	#[derive(Clone, Copy, PartialEq, Eq, Debug, Encode, Decode, MaxEncodedLen, TypeInfo)]
	pub enum RewardKind {
		Enabled,
		Disabled,
	}

	#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, MaxEncodedLen, TypeInfo)]
	pub enum Candidate<JudgementId, Deposit, BlockNumber, AccountId> {
		Applied {
			cred: Credibility<Deposit, PersonalId, AccountId>,
			entropy: Entropy,
			entropy_since: BlockNumber,
		},
		Selected {
			since: BlockNumber,
			cred: Credibility<Deposit, PersonalId, AccountId>,
			reserved: PersonalId,
			entropy: Entropy,
			design: InkSpec,
			allocation: Allocation,
			judging: Option<JudgementId>,
			failed: Counter,
		},
		Proven {
			design: InkSpec,
			reserved: PersonalId,
			was_referred: bool,
			was_invited: bool,
		},
	}

	#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, MaxEncodedLen, TypeInfo)]
	pub struct Person<BoundedAccounts: Default> {
		pub(super) design: Option<InkSpec>,
		/// An unordered list of active referrals this `Person` is responsible for.
		pub(super) active_referrals: BoundedAccounts,
		/// The number of referral tickets the person is allowed to issue.
		/// Increments upon successful referal registration.
		pub(super) allowed_referral_tickets: Counter,
		pub bad_referrals: Counter,
		pub(super) successful_referrals: Counter,
		pub(super) referrals: Counter,
		pub(super) derivatives: Counter,
		pub(super) banned: bool,
		/// Referral rewards from successful referrals ready to be registered.
		pub(super) pending_referral_rewards: Counter,
	}

	/// Information for a referral ticket.
	#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, MaxEncodedLen, TypeInfo)]
	pub struct ReferralTicket<Ticket> {
		/// The referral ticket.
		pub ticket: Ticket,
	}

	/// The current personal identities which are going through the proof-of-ink process.
	#[pallet::storage]
	pub type Candidates<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, CandidateOf<T>>;

	/// The personal identities which we track; all which have been recognized through proof-of-ink
	/// are in here.
	#[pallet::storage]
	pub type People<T: Config> = StorageMap<
		_,
		Twox64Concat,
		PersonalId,
		Person<BoundedVec<T::AccountId, T::MaxActiveReferrals>>,
	>;

	/// The tickets stored on-chain ready to be used to refer candidates.
	#[pallet::storage]
	pub type ReferralTickets<T: Config> = StorageMap<
		_,
		Twox64Concat,
		PersonalId,
		BoundedVec<ReferralTicket<T::Ticket>, T::MaxActiveReferrals>,
	>;

	/// The current design families we recognise.
	#[pallet::storage]
	pub type DesignFamilies<T> = StorageMap<_, Blake2_128Concat, FamilyIndex, Family>;

	/// The committed designs which are no longer available.
	#[pallet::storage]
	pub type CommittedDesigns<T> = StorageDoubleMap<
		_,
		Blake2_128Concat,
		FamilyIndex,
		Blake2_128Concat,
		DesignIndex,
		DesignStatus,
	>;

	/// The number of judgements currently ongoing.
	#[pallet::storage]
	pub type AllocationCount<T> = StorageValue<_, Counter, ValueQuery>;

	/// The configuration.
	#[pallet::storage]
	pub type Configuration<T> = StorageValue<_, ConfigRecord<BlockNumberFor<T>>, ValueQuery>;

	/// Number of invitations available to distribute for an account.
	#[pallet::storage]
	pub type AvailableInvites<T: Config> =
		StorageMap<_, Blake2_128Concat, T::AccountId, u32, ValueQuery>;

	/// Pending invitations for each inviter (an account that invites others).
	///
	/// If the key `(inviter, ticket)` exists, then the ticket is currently a pending invite.
	#[pallet::storage]
	pub(crate) type PendingInvites<T: Config> =
		StorageDoubleMap<_, Blake2_128Concat, T::AccountId, Blake2_128Concat, TicketOf<T>, ()>;

	/// The values of the reimbursement awarded to referrers, along with how many of each value
	/// should be awarded. These values are stored in reverse order of their priority, so the last
	/// value in the list will be the first one to be used by the reimbursement system.
	///
	/// Storage item name keeps the legacy prefix for storage readability compatibility.
	#[pallet::storage]
	pub type ReferrerReimbursementValues<T: Config> =
		StorageValue<_, BoundedVec<(BalanceOf<T>, u32), T::MaxReimbursementValues>>;

	/// The values of the reimbursement awarded to referred people, along with how many of each
	/// value should be awarded. These values are stored in reverse order of their priority, so the
	/// last value in the list will be the first one to be used by the reimbursement system.
	///
	/// Storage item name keeps the legacy prefix for storage readability compatibility.
	#[pallet::storage]
	pub type ReferredReimbursementValues<T: Config> =
		StorageValue<_, BoundedVec<(BalanceOf<T>, u32), T::MaxReimbursementValues>>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// Candidate applied for verification.
		CandidateApplied { account_id: T::AccountId },
		/// Candidate opened a Mob Rule case for their verification evidence.
		JudgementRequested { account_id: T::AccountId },
		/// Oracle has provided the judgement for a Mob Rule case.
		JudgementProvided { account_id: T::AccountId, judgement: Judgement },
		/// A candidate has been granted a free retry attempt after a failure to verify their
		/// evidence.
		RetryGranted { account_id: T::AccountId, failures: u32 },
		/// Register an account as a person.
		PersonRegistered { account_id: T::AccountId, personal_id: PersonalId },
		/// Person referred an account.
		CandidateReferred { referrer: PersonalId, referred: T::AccountId },
		/// Entropy for a candidate changed after expiry.
		Rerolled { account_id: T::AccountId },
		/// Candidate committed to an ink design.
		DesignCommitted { account_id: T::AccountId, reserved_id: PersonalId },
		/// Storage fully allocated for a committed design.
		FullyAllocated { account_id: T::AccountId },
		/// Candidate removed after timeout.
		TimedOut { account_id: T::AccountId },
		/// Uncommitted candidate removed.
		FlakedOut { account_id: T::AccountId },
		/// Referral ticket created by storing the public key on chain.
		TicketReferred { referrer: PersonalId, ticket: T::Ticket },
		/// Referral ticket removed by removing the public key from chain storage.
		TicketCancelled { referrer: PersonalId, ticket: T::Ticket },
		/// Candidate applied using a referral ticket.
		TicketApplied { account_id: T::AccountId, referrer: PersonalId },
		/// Design family added.
		FamilyAdded { index: FamilyIndex, kind: FamilyKind, id: FamilyId },
		/// All invites have been removed for the inviter.
		AllInvitesRemoved { inviter: T::AccountId },
		/// Some invites have been removed for the inviter, some are remaining.
		SomeInvitesRemoved { inviter: T::AccountId },
		/// Candidate applied using an invitation.
		InvitedCandidateApplied { who: T::AccountId, inviter: T::AccountId },
		/// A referrer's reward voucher has been registered.
		ReferralVoucherRegistered { referrer: PersonalId },
		/// Invites have been granted to an account.
		InvitesGranted { account: T::AccountId, count: u32 },
		/// An invite ticket has been set.
		InviteTicketSet { inviter: T::AccountId, ticket: T::Ticket },
		/// An invite ticket has been cancelled.
		InviteTicketCancelled { inviter: T::AccountId, ticket: T::Ticket },
		/// The pallet configuration has been updated.
		ConfigurationSet { config: ConfigRecord<BlockNumberFor<T>> },
	}

	#[pallet::error]
	pub enum Error<T> {
		/// Account is already applying to make a proof-of-ink.
		InProgress,
		/// Account has not been referred.
		NoReferral,
		/// The callback context is invalid; this should never happen.
		BadContext,
		/// The incoming judgement ID is not what we were expecting for the account.
		UnexpectedJudgement,
		/// No arguments were supplied with the judgement.
		NoArgs,
		/// The account has not applied to make a proof-of-ink.
		NotApplied,
		/// The candidate has not committed to a design.
		NotSelected,
		/// The candidate did not prove themselves yet.
		NotProven,
		/// The candidate has already started their judgement.
		AlreadyStarted,
		/// The personal ID is not in range.
		OutOfRange,
		/// The personal ID has already been taken.
		AlreadyTaken,
		/// The person has no more referrals left to give.
		NoMoreReferrals,
		/// The reroll is too early.
		TooEarly,
		/// The referrer's design is not procedural.
		DesignInvalid,
		/// The referrer's design is already taken.
		DesignTaken,
		/// The referrer doesn't appear to be a person. This should never happen.
		BadParent,
		/// The design family doesn't exist.
		BadFamily,
		/// The design family is invalid for this choice.
		WrongFamily,
		/// The index of the design or variant is beyond the allowed maximum for the family.
		IndexTooBig,
		/// The system is busy with too many commitments; try again later.
		Busy,
		/// The person has been banned from referring.
		Banned,
		/// The candidate has not demonstrated probably through the initial evidence.
		Improbable,
		/// The personal identity has already been reserved.
		IdReserved,
		/// The personal identity is already taken by a proven person.
		IdUsed,
		/// The ticket provided is invalid.
		InvalidTicket,
		/// The caller has not provided a referral or invitation ticket.
		NoTicket,
		/// The account is not authorized to do this.
		NotAuthorized,
		/// The personal identity doesn't exist.
		NotPerson,
		/// The candidate is referred. But the operation requires the candidate to not be referred.
		ReferredCandidate,
		/// The candidate is not referred. But the operation requires the candidate to be referred.
		NotReferredCandidate,
		/// There is no referral reward to register.
		NoRewardToRegister,
		/// There is a pending referral reward that must be registered first.
		RewardToRegister,
		/// No inviter found.
		NoInviter,
		/// Invalid signature.
		InvalidSignature,
		/// No invite available.
		NoInvites,
		/// Invite is already set.
		AlreadyInvited,
		/// The referrer is not a person.
		NoReferrer,
		/// The individual is not a person recognized by proof of ink.
		NotPoiPerson,
		/// The proof of ownership is invalid.
		InvalidProofOfOwnership,
		/// The reimbursement values are invalid.
		InvalidReimbursementValues,
	}

	/// A reason for this pallet placing a hold on funds.
	#[pallet::composite_enum]
	pub enum HoldReason {
		/// The funds are held as storage deposit for personhood candidacy.
		ProofOfInk,
	}

	type CallbackOf<T> = Callback<
		(OracleTicketOf<T>, JudgementContext, Judgement),
		<T as frame_system::Config>::RuntimeCall,
	>;

	/// Namespacing for getting access to the callback values.
	///
	/// Callbacks must be functions since they need pallet index which is not `const`.
	/// Expected usage outside of the pallet would be:
	/// `use pallet_other::Callbacks; pallet_other::Call::<T>::dispatchable()`.
	// This could be moved into a macro.
	pub trait Callbacks {
		type T: Config;
		fn judged() -> CallbackOf<Self::T>;
	}
	impl<T: Config> Callbacks for Call<T> {
		type T = T;
		fn judged() -> CallbackOf<T> {
			use frame_support::traits::GetCallIndex;
			// Parameters to this call do not matter, as we are just extracting the call index.
			let judgement_call = Call::<T>::judged {
				ticket: Default::default(),
				context: Default::default(),
				judgement: Judgement::Contempt,
			};
			let call_index = judgement_call.get_call_index();
			Callback::from_parts(Pallet::<T>::index() as u8, call_index)
		}
	}

	#[pallet::origin]
	#[derive(
		CloneNoBound,
		PartialEqNoBound,
		EqNoBound,
		DebugNoBound,
		Encode,
		Decode,
		MaxEncodedLen,
		TypeInfo,
		DecodeWithMemTracking,
	)]
	#[scale_info(skip_type_params(T))]
	pub enum Origin<T: Config> {
		/// This account is authorized to apply with signature.
		/// * account is not a candidate yet,
		/// * referrer is a person,
		/// * referrer is not banned,
		/// * referral ticket exists,
		/// * ticket signature is valid.
		AuthorizedApplyWithSig(T::AccountId),
		ReferredCandidate(T::AccountId),
		InvitedCandidate(T::AccountId),
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn integrity_test() {
			assert!(
				<T as Config>::MaxActiveReferrals::get() <= 200,
				"maximum number of active referrals cannot exceed 200"
			);
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// - Declare intention to get a tattoo.
		/// - Deposit is taken.
		/// - `InkType` may not be `ProceduralDerivative`.
		#[pallet::weight(T::WeightInfo::apply())]
		#[pallet::call_index(0)]
		pub fn apply(origin: OriginFor<T>) -> DispatchResultWithPostInfo {
			let account = ensure_signed(origin)?;
			ensure!(!Candidates::<T>::contains_key(&account), Error::<T>::InProgress);

			let footprint = Self::apply_footprint();
			let deposit = T::Deposit::new(&account, footprint)?;
			let status = Candidate::Applied {
				cred: Credibility::Deposit(deposit),
				entropy: (b"poi/apply", &account).using_encoded(|s| T::Randomness::random(s).0),
				entropy_since: frame_system::Pallet::<T>::block_number(),
			};
			Candidates::<T>::insert(&account, &status);
			Self::deposit_event(Event::CandidateApplied { account_id: account });
			Ok(Pays::Yes.into())
		}

		/// - Open a Mob Rule case to judge the `evidence`.
		/// - Requests a judgement for the proof-of-ink evidence statement.
		/// - Needs `SignedExtension` to avoid upfront requirement for fee if `judgements == 0`.
		/// - If `judgements > 0`, then an additional fee should be charged into Treasury.
		#[pallet::weight({
			let mut w = T::WeightInfo::submit_evidence(0, 0);
			for c in 0..4 {
				for a in 0..2 {
					w = w.max(T::WeightInfo::submit_evidence(c, a));
				}
			}
			w
		})]
		#[pallet::call_index(1)]
		pub fn submit_evidence(
			origin: OriginFor<T>,
			evidence: EvidenceHash,
		) -> DispatchResultWithPostInfo {
			let account = ensure_credible::<T>(origin)?;
			let status = Candidates::<T>::get(&account).ok_or(Error::<T>::NotApplied)?;
			let Candidate::Selected {
				since,
				cred,
				reserved,
				entropy,
				design,
				allocation,
				failed,
				judging,
			} = status
			else {
				Err(Error::<T>::NotSelected)?
			};
			ensure!(judging.is_none(), Error::<T>::AlreadyStarted);
			let context = JudgementContext::truncate_from(account.encode());
			let callback = Call::<T>::judged();
			let probable_acceptable = allocation == Allocation::Initial;
			let statement = ProofOfInk { design: design.clone(), evidence, probable_acceptable };
			let id = T::Oracle::judge_statement(statement, context, callback)?;
			let judging = Some(id);
			let status = Candidate::Selected {
				cred,
				entropy,
				reserved,
				since,
				design,
				allocation,
				failed,
				judging,
			};
			Candidates::<T>::insert(&account, &status);
			Self::deposit_event(Event::JudgementRequested { account_id: account });
			if failed == 0 {
				return Ok(Pays::No.into());
			}
			Ok(().into())
		}

		/// Is called by the Oracle when the evidence has been judged.
		#[pallet::weight(T::WeightInfo::judged(T::MaxActiveReferrals::get() - 1))]
		#[pallet::call_index(2)]
		pub fn judged(
			origin: OriginFor<T>,
			ticket: OracleTicketOf<T>,
			context: JudgementContext,
			judgement: Judgement,
		) -> DispatchResultWithPostInfo {
			ensure_root(origin)?;
			let account =
				T::AccountId::decode(&mut &context[..]).map_err(|_| Error::<T>::BadContext)?;
			let status = Candidates::<T>::get(&account).ok_or(Error::<T>::BadContext)?;
			let Candidate::Selected {
				cred,
				entropy,
				reserved,
				since,
				design,
				allocation,
				mut failed,
				mut judging,
			} = status
			else {
				Err(Error::<T>::BadContext)?
			};
			let Some(id) = judging.take() else { Err(Error::<T>::BadContext)? };
			ensure!(id == ticket, Error::<T>::UnexpectedJudgement);

			let initial_referral_count = match cred {
				Credibility::Referred(who) => People::<T>::get(who)
					.map(|record| record.active_referrals.len() as u32)
					.unwrap_or(0u32),
				_ => 0,
			};
			let actual_weight = T::WeightInfo::judged(initial_referral_count.saturating_sub(1));

			let status = match (judgement, &allocation) {
				(Judgement::Contempt, _) => {
					match cred {
						Credibility::Referred(who) => Self::referral_contemptuous(who, &account),
						Credibility::Deposit(ticket) => ticket.burn(&account),
						Credibility::Invited(_) => {
							let _ = frame_system::Pallet::<T>::dec_sufficients(&account);
						},
					}
					T::People::cancel_id_reservation(reserved)?;
					AllocationCount::<T>::mutate(|n| n.saturating_dec());
					Candidates::<T>::remove(&account);
					if let InkSpec::DesignedElective(family_id, design_index) = design {
						CommittedDesigns::<T>::remove(family_id, design_index);
					}
					Self::deposit_event(Event::JudgementProvided {
						account_id: account,
						judgement,
					});
					return Ok(PostDispatchInfo {
						actual_weight: Some(actual_weight),
						pays_fee: Pays::No,
					})
				},
				(Judgement::Truth(False), _) | (_, Allocation::InitDone) => {
					// Note the failed attempt.
					failed.saturating_inc();
					if failed > T::MaxRetryAttempts::get() {
						// No more retries allowed.
						match cred {
							Credibility::Referred(who) => Self::referral_bad(who, &account),
							Credibility::Deposit(ticket) => ticket.burn(&account),
							Credibility::Invited(_) => {
								let _ = frame_system::Pallet::<T>::dec_sufficients(&account);
							},
						}
						T::People::cancel_id_reservation(reserved)?;
						AllocationCount::<T>::mutate(|n| n.saturating_dec());
						Candidates::<T>::remove(&account);
						if let InkSpec::DesignedElective(family_id, design_index) = design {
							CommittedDesigns::<T>::remove(family_id, design_index);
						}
						Self::deposit_event(Event::JudgementProvided {
							account_id: account,
							judgement,
						});
						return Ok(PostDispatchInfo {
							actual_weight: Some(actual_weight),
							pays_fee: Pays::No,
						})
					}

					Self::deposit_event(Event::RetryGranted {
						account_id: account.clone(),
						failures: failed,
					});
					// Allow a new submission.
					T::DataStore::refresh_allocation(&account)?;
					Candidate::Selected {
						cred,
						entropy,
						reserved,
						since,
						design,
						allocation,
						failed,
						judging,
					}
				},
				(Judgement::Truth(True), Allocation::Initial) => {
					// Initial proof ok - allow allocation of the full amount now.
					let allocation = Allocation::InitDone;
					Candidate::Selected {
						cred,
						entropy,
						reserved,
						since,
						design,
						allocation,
						failed,
						judging,
					}
				},
				(Judgement::Truth(True), Allocation::Full) => {
					AllocationCount::<T>::mutate(|n| n.saturating_dec());
					let was_referred = matches!(&cred, Credibility::Referred(_));
					let was_invited = matches!(&cred, Credibility::Invited(_));
					match cred {
						Credibility::Referred(who) => Self::referral_ok(who, &account),
						Credibility::Deposit(ticket) => ticket.drop(&account).unwrap_or_default(),
						// We don't decrease the sufficient provider for the target yet.
						// It will be done when the candidate registers their key.
						Credibility::Invited(_) => (),
					}
					if let InkSpec::DesignedElective(family_id, design_index) = design {
						CommittedDesigns::<T>::insert(
							family_id,
							design_index,
							DesignStatus::Committed,
						);
					}
					Candidate::Proven { design, reserved, was_referred, was_invited }
				},
			};

			Candidates::<T>::insert(&account, &status);
			Self::deposit_event(Event::JudgementProvided { account_id: account, judgement });
			Ok(PostDispatchInfo { actual_weight: Some(actual_weight), pays_fee: Pays::No })
		}

		/// - Gets a new `index`
		/// - Puts an entry into `People`.
		/// - Calls People/`newPerson(index, key)`
		///
		/// # Arguments
		/// * `key` - The personal identity to register.
		/// * `destination` - The account receiving the candidate's referral reward.
		/// * `proof_of_ownerhsip` - The signature of predefined prefix `"pop register using"`
		///   concatenated with the sender account id.
		#[pallet::weight(T::WeightInfo::register_referred())]
		#[pallet::call_index(3)]
		pub fn register_referred(
			origin: OriginFor<T>,
			key: MemberOf<T>,
			destination: T::AccountId,
			proof_of_ownership: <T::Crypto as GenerateVerifiable>::Signature,
		) -> DispatchResultWithPostInfo {
			let account = ensure_signed_or_referred::<T>(origin)?;
			let status = Candidates::<T>::get(&account).ok_or(Error::<T>::NotApplied)?;
			let Candidate::Proven { design, reserved: id, was_referred, was_invited: _ } = status
			else {
				Err(Error::<T>::NotProven)?
			};
			ensure!(was_referred, Error::<T>::NotReferredCandidate);
			Self::register_proven_candidate(
				account.clone(),
				id,
				key,
				design,
				ReferralReward::Destination(destination),
				RewardKind::Disabled,
				proof_of_ownership,
			)?;
			let _ = frame_system::Pallet::<T>::dec_sufficients(&account);
			Ok(Pays::No.into())
		}

		/// - Gets a new `index`
		/// - Puts an entry into `People`.
		/// - Calls People/`newPerson(index, key)`
		///
		/// # Arguments
		/// * `key` - The personal identity to register.
		/// * `destination` - The account receiving rewards.
		/// * `proof_of_ownerhsip` - The signature of predefined prefix `"pop register using"`
		///   concatenated with the sender account id.
		///
		/// # Warning:
		///
		/// This call can pay two rewards to the same account for non-referred candidates.
		/// The app is responsible for any additional privacy handling after transfer.
		#[pallet::weight(T::WeightInfo::register_non_referred())]
		#[pallet::call_index(4)]
		pub fn register_non_referred(
			origin: OriginFor<T>,
			key: MemberOf<T>,
			destination: T::AccountId,
			proof_of_ownership: <T::Crypto as GenerateVerifiable>::Signature,
		) -> DispatchResultWithPostInfo {
			let account = ensure_signed_or_invited::<T>(origin)?;
			let status = Candidates::<T>::get(&account).ok_or(Error::<T>::NotApplied)?;
			let Candidate::Proven { design, reserved: id, was_referred, was_invited } = status
			else {
				Err(Error::<T>::NotProven)?
			};
			ensure!(!was_referred, Error::<T>::ReferredCandidate);
			if was_invited {
				let _ = frame_system::Pallet::<T>::dec_sufficients(&account);
			}
			Self::register_proven_candidate(
				account,
				id,
				key,
				design,
				ReferralReward::Destination(destination),
				RewardKind::Enabled,
				proof_of_ownership,
			)?;
			Ok(Pays::No.into())
		}

		// Change entropy after timeout from `entropy_since`.
		#[pallet::weight(T::WeightInfo::reroll())]
		#[pallet::call_index(5)]
		pub fn reroll(origin: OriginFor<T>) -> DispatchResultWithPostInfo {
			let account = ensure_credible::<T>(origin)?;
			let status = Candidates::<T>::get(&account).ok_or(Error::<T>::NotApplied)?;
			let err = Err(Error::<T>::NotApplied);
			let Candidate::Applied { cred, entropy_since, .. } = status else { err? };
			let expiry = entropy_since.saturating_add(Configuration::<T>::get().reroll_timeout);
			let now = frame_system::Pallet::<T>::block_number();
			ensure!(expiry <= now, Error::<T>::TooEarly);
			let entropy = (b"poi/apply", &account).using_encoded(|s| T::Randomness::random(s).0);
			let entropy_since = expiry;
			let status = Candidate::Applied { cred, entropy, entropy_since };
			Candidates::<T>::insert(&account, &status);
			Self::deposit_event(Event::Rerolled { account_id: account });
			Ok(Pays::No.into())
		}

		/// Commit to a design and authorize the storage for (possibly just initial) evidence.
		///
		/// If a specific personal identity is required, then this can be placed in `require_id`.
		/// This can be any unused/unreserved personal identity no greater than the `NextId`
		/// counter.
		#[pallet::weight({
			let c = match choice {
				InkChoice::DesignedElective(_, _) => 0,
				InkChoice::ProceduralAccount(_) => 1,
				InkChoice::ProceduralPersonal(_) => 2,
				InkChoice::Procedural(_, _) => 3,
				InkChoice::ProceduralDerivative(_, _) => 4,
			};

			T::WeightInfo::commit(c, 0).max(T::WeightInfo::commit(c, 1))
		})]
		#[pallet::call_index(6)]
		pub fn commit(
			origin: OriginFor<T>,
			choice: InkChoice,
			require_id: Option<PersonalId>,
		) -> DispatchResultWithPostInfo {
			let account = ensure_credible::<T>(origin)?;
			let status = Candidates::<T>::get(&account).ok_or(Error::<T>::NotApplied)?;
			let err = Err(Error::<T>::NotApplied);
			let Candidate::Applied { cred, entropy, .. } = status else { err? };
			let config = Configuration::<T>::get();
			let alloc_count = AllocationCount::<T>::get();
			ensure!(alloc_count < config.maximum, Error::<T>::Busy);

			let id = if let Some(require_id) = require_id {
				T::People::renew_id_reservation(require_id)?;
				require_id
			} else {
				T::People::reserve_new_id()
			};
			let design = Self::bake_design(choice, entropy, account.clone(), id)?;
			if let InkSpec::DesignedElective(family_id, design_index) = design {
				CommittedDesigns::<T>::insert(family_id, design_index, DesignStatus::Reserved);
			}

			let (allocation, len, count) = if alloc_count < config.fasttrack_count {
				// Full (64MB) storage allocation
				(Allocation::Full, config.full_alloc_len, config.full_alloc_count)
			} else {
				// Just an initial (2MB) storage allocation
				(Allocation::Initial, config.init_alloc_len, config.init_alloc_count)
			};

			T::DataStore::allocate_storage(&account, len, count)?;
			AllocationCount::<T>::put(alloc_count.saturating_add(1));
			let status = Candidate::Selected {
				since: frame_system::Pallet::<T>::block_number(),
				cred,
				reserved: id,
				entropy,
				design,
				allocation,
				judging: None,
				failed: 0,
			};
			Candidates::<T>::insert(&account, status);
			Self::deposit_event(Event::DesignCommitted { account_id: account, reserved_id: id });
			Ok(Pays::No.into())
		}

		// Commit to a design and authorize the storage for (possibly just initial) evidence.
		#[pallet::weight(T::WeightInfo::allocate_full())]
		#[pallet::call_index(7)]
		pub fn allocate_full(origin: OriginFor<T>) -> DispatchResultWithPostInfo {
			let account = ensure_credible::<T>(origin)?;
			let status = Candidates::<T>::get(&account).ok_or(Error::<T>::NotApplied)?;
			let Candidate::Selected {
				cred,
				entropy,
				reserved,
				since,
				design,
				allocation,
				failed,
				judging,
			} = status
			else {
				Err(Error::<T>::BadContext)?
			};
			ensure!(allocation == Allocation::InitDone, Error::<T>::Improbable);

			let config = Configuration::<T>::get();
			let allocation = Allocation::Full;
			let len = config.full_alloc_len.saturating_sub(config.init_alloc_len);
			let count = config.full_alloc_count.saturating_sub(config.init_alloc_count);

			T::DataStore::allocate_storage(&account, len, count)?;
			let status = Candidate::Selected {
				since,
				cred,
				reserved,
				entropy,
				design,
				allocation,
				judging,
				failed,
			};
			Candidates::<T>::insert(&account, status);
			Self::deposit_event(Event::FullyAllocated { account_id: account });
			Ok(Pays::No.into())
		}

		/// Once a timeout passes, removes a candidate who is selected but not yet proven.
		/// If the candidate was referred then referral is bad, if candidate applied with deposit,
		/// deposit is slashed.
		#[pallet::weight(T::WeightInfo::timeout())]
		#[pallet::call_index(8)]
		pub fn timeout(origin: OriginFor<T>, account: T::AccountId) -> DispatchResultWithPostInfo {
			ensure_signed(origin)?;
			let status = Candidates::<T>::get(&account).ok_or(Error::<T>::NotApplied)?;
			let Candidate::Selected { cred, since, reserved, design, .. } = status else {
				Err(Error::<T>::BadContext)?
			};
			let now = frame_system::Pallet::<T>::block_number();
			let config = Configuration::<T>::get();
			ensure!(now > since + config.timeout, Error::<T>::TooEarly);

			match cred {
				Credibility::Referred(who) => Self::referral_bad(who, &account),
				Credibility::Deposit(ticket) => ticket.burn(&account),
				Credibility::Invited(_) => {
					let _ = frame_system::Pallet::<T>::dec_sufficients(&account);
				},
			}
			AllocationCount::<T>::mutate(|n| n.saturating_dec());
			T::People::cancel_id_reservation(reserved)?;
			Candidates::<T>::remove(&account);
			if let InkSpec::DesignedElective(family_id, design_index) = design {
				CommittedDesigns::<T>::remove(family_id, design_index);
			}
			Self::deposit_event(Event::TimedOut { account_id: account });

			Ok(Pays::No.into())
		}

		/// Remove a candidate who has not yet committed.
		#[pallet::weight(T::WeightInfo::flakeout(T::MaxActiveReferrals::get() - 1))]
		#[pallet::call_index(9)]
		pub fn flakeout(origin: OriginFor<T>) -> DispatchResultWithPostInfo {
			let account = ensure_credible::<T>(origin)?;
			let status = Candidates::<T>::get(&account).ok_or(Error::<T>::NotApplied)?;
			let Candidate::Applied { cred, .. } = status else { Err(Error::<T>::BadContext)? };

			let initial_referral_count = match cred {
				Credibility::Referred(who) => People::<T>::get(who)
					.map(|record| record.active_referrals.len() as u32)
					.unwrap_or(0u32),
				_ => 0,
			};
			match cred {
				Credibility::Referred(who) => Self::referral_flaked(who, &account),
				Credibility::Deposit(deposit) => {
					let _ = deposit.drop(&account);
				},
				Credibility::Invited(_) => {
					let _ = frame_system::Pallet::<T>::dec_sufficients(&account);
				},
			}
			Candidates::<T>::remove(&account);
			Self::deposit_event(Event::FlakedOut { account_id: account });

			let actual_weight = T::WeightInfo::flakeout(initial_referral_count.saturating_sub(1));
			Ok(PostDispatchInfo { actual_weight: Some(actual_weight), pays_fee: Pays::Yes })
		}

		/// Utilize a referral signature.
		///
		/// The payload to be signed by the referrer using their registered key pair is the encoded
		/// bytes of the account ID of the candidate.
		#[pallet::weight(T::WeightInfo::apply_with_signature())]
		#[pallet::call_index(10)]
		pub fn apply_with_signature(
			origin: OriginFor<T>,
			referrer: PersonalId,
			signature: T::TicketSignature,
			ticket: T::Ticket,
		) -> DispatchResultWithPostInfo {
			let Ok(Origin::AuthorizedApplyWithSig(account)) = origin.into() else {
				return Err(DispatchError::BadOrigin.into());
			};

			let mut tickets = ReferralTickets::<T>::get(referrer).ok_or(Error::<T>::NoTicket)?;

			let ticket_pos = tickets
				.iter()
				.enumerate()
				.find_map(|(pos, t)| {
					if t.ticket == ticket &&
						account.using_encoded(|bytes| signature.verify(bytes, &t.ticket))
					{
						Some(pos)
					} else {
						None
					}
				})
				.ok_or(Error::<T>::NoTicket)?;

			tickets.remove(ticket_pos);
			if tickets.is_empty() {
				ReferralTickets::<T>::remove(referrer);
			} else {
				ReferralTickets::<T>::insert(referrer, tickets);
			}

			let status = Candidate::Applied {
				cred: Credibility::Referred(referrer),
				entropy: (b"poi/apply", &account).using_encoded(|s| T::Randomness::random(s).0),
				entropy_since: frame_system::Pallet::<T>::block_number(),
			};

			let mut referrer_record = People::<T>::get(referrer)
				.defensive() // checked by the origin
				.ok_or(Error::<T>::NoReferrer)?;

			// The transaction extension checks whether the origin is already a candidate,
			// so if multiple tickets were given to a single origin, they would just be wasted since
			// the origin will only be able to apply once.
			referrer_record
				.active_referrals
				.try_push(account.clone())
				// This should not happen because the limit of unique tickets a referrer can give
				// away is equal to the limit of active referrals.
				.map_err(|_| Error::<T>::NoMoreReferrals)?;
			referrer_record.referrals.saturating_inc();

			Candidates::<T>::insert(&account, &status);
			People::<T>::insert(referrer, referrer_record);
			frame_system::Pallet::<T>::inc_sufficients(&account);
			Self::deposit_event(Event::TicketApplied { account_id: account, referrer });
			Ok(Pays::No.into())
		}

		/// Apply to the Proof-of-Ink process with an invitation.
		///
		/// The payload to be signed by the inviter using their registered key pair is the encoded
		/// bytes of the account ID of the candidate.
		#[pallet::weight(T::WeightInfo::apply_with_invitation())]
		#[pallet::call_index(19)]
		pub fn apply_with_invitation(
			origin: OriginFor<T>,
			inviter: T::AccountId,
			ticket: TicketOf<T>,
			_signature: T::TicketSignature, // validated in transaction extension
		) -> DispatchResultWithPostInfo {
			let Ok(Origin::InvitedCandidate(account)) = origin.into() else {
				return Err(DispatchError::BadOrigin.into());
			};

			PendingInvites::<T>::take(&inviter, &ticket)
				.defensive()
				.ok_or(Error::<T>::NoTicket)?;

			let status = Candidate::Applied {
				cred: Credibility::Invited(ticket),
				entropy: (b"poi/apply", &account).using_encoded(|s| T::Randomness::random(s).0),
				entropy_since: frame_system::Pallet::<T>::block_number(),
			};

			Candidates::<T>::insert(&account, &status);
			frame_system::Pallet::<T>::inc_sufficients(&account);
			Self::deposit_event(Event::InvitedCandidateApplied { who: account, inviter });
			Ok(Pays::No.into())
		}

		/// Add a design family. Must have privileged access to do this.
		#[pallet::weight(T::WeightInfo::add_design_family())]
		#[pallet::call_index(11)]
		pub fn add_design_family(
			origin: OriginFor<T>,
			index: FamilyIndex,
			kind: FamilyKind,
			id: FamilyId,
		) -> DispatchResultWithPostInfo {
			T::ManagerOrigin::ensure_origin_or_root(origin)?;
			DesignFamilies::<T>::insert(index, Family { kind: kind.clone(), id });
			Self::deposit_event(Event::FamilyAdded { index, kind, id });
			Ok(Pays::No.into())
		}

		/// Add a referral ticket associated with a person.
		///
		/// Only one referral ticket may be active at any given time for one person. Calling this
		/// extrinsic while a valid ticket is set will overwrite the existing ticket.
		///
		/// If any pending referral rewards are present, they need to be registered first.
		#[pallet::weight(T::WeightInfo::set_referral_ticket())]
		#[pallet::call_index(12)]
		pub fn set_referral_ticket(
			origin: OriginFor<T>,
			ticket: T::Ticket,
		) -> DispatchResultWithPostInfo {
			let referrer = T::EnsurePerson::ensure_origin(origin)?;

			let referrer_record = People::<T>::get(referrer).ok_or(Error::<T>::NotPoiPerson)?;
			ensure!(!referrer_record.banned, Error::<T>::Banned);
			ensure!(referrer_record.pending_referral_rewards == 0, Error::<T>::RewardToRegister);
			ensure!(!referrer_record.active_referrals.is_full(), Error::<T>::NoMoreReferrals);

			if let Some(tickets_issued) = ReferralTickets::<T>::get(referrer) {
				ensure!(
					referrer_record.allowed_referral_tickets > tickets_issued.len() as u32,
					Error::<T>::NoMoreReferrals
				);
				ensure!(
					tickets_issued.len().saturating_add(referrer_record.active_referrals.len()) <
						T::MaxActiveReferrals::get() as usize,
					Error::<T>::NoMoreReferrals
				);
			}

			let mut pays = Pays::No;
			let ticket_record = ReferralTicket { ticket: ticket.clone() };

			ReferralTickets::<T>::try_mutate(referrer, |maybe_tickets| -> Result<(), Error<T>> {
				if let Some(tickets) = maybe_tickets {
					if tickets.contains(&ticket_record.clone()) {
						pays = Pays::Yes;
						return Ok(());
					};

					tickets.try_push(ticket_record).map_err(|_| Error::<T>::NoMoreReferrals)?;
				} else {
					*maybe_tickets = Some(
						BoundedVec::<ReferralTicket<T::Ticket>, T::MaxActiveReferrals>::try_from(
							vec![ticket_record],
						)
						.map_err(|_| Error::<T>::NoMoreReferrals)?,
					);
				}
				Ok(())
			})?;

			// Event only deposited if a ticket was indeed set
			if pays == Pays::No {
				Self::deposit_event(Event::TicketReferred { referrer, ticket });
			}

			Ok(pays.into())
		}

		/// Cancel a referral ticket associated with a person.
		#[pallet::weight(T::WeightInfo::cancel_referral_ticket())]
		#[pallet::call_index(13)]
		pub fn cancel_referral_ticket(
			origin: OriginFor<T>,
			ticket: T::Ticket,
		) -> DispatchResultWithPostInfo {
			let referrer = T::EnsurePerson::ensure_origin(origin)?;

			let referrer_record = People::<T>::get(referrer).ok_or(Error::<T>::NotPoiPerson)?;
			ensure!(!referrer_record.banned, Error::<T>::Banned);

			ensure!(ReferralTickets::<T>::get(referrer).is_some(), Error::<T>::NoTicket);

			let ticket_record = ReferralTicket { ticket: ticket.clone() };

			ReferralTickets::<T>::try_mutate(referrer, |maybe_tickets| -> Result<(), Error<T>> {
				if let Some(tickets) = maybe_tickets {
					if !tickets.contains(&ticket_record.clone()) {
						return Err(Error::<T>::NoTicket);
					};

					tickets.retain(|t| t.ticket != ticket_record.ticket);
				} else {
					return Err(Error::<T>::NoTicket);
				}
				Ok(())
			})?;

			Self::deposit_event(Event::TicketCancelled { referrer, ticket });
			Ok(Pays::No.into())
		}

		#[pallet::weight(T::WeightInfo::register_successful_referral_reward())]
		#[pallet::call_index(14)]
		pub fn register_successful_referral_reward(
			origin: OriginFor<T>,
			destination: T::AccountId,
		) -> DispatchResultWithPostInfo {
			let referrer = T::EnsurePerson::ensure_origin(origin)?;
			let mut referrer_record = People::<T>::get(referrer).ok_or(Error::<T>::NotPoiPerson)?;
			ensure!(!referrer_record.banned, Error::<T>::Banned);
			referrer_record.pending_referral_rewards = referrer_record
				.pending_referral_rewards
				.checked_sub(1)
				.ok_or(Error::<T>::NoRewardToRegister)?;

			Self::register_referrer_reward(destination)?;

			People::<T>::insert(referrer, referrer_record);

			Self::deposit_event(Event::ReferralVoucherRegistered { referrer });
			Ok(Pays::No.into())
		}

		/// Grants invites to an account so they can distribute them.
		///
		/// The origin must be `InvitationsOrigin.
		///
		/// - `account`: The account to give invites to.
		/// - `count`: The number of invites to give.
		#[pallet::call_index(15)]
		#[pallet::weight(T::WeightInfo::grant_invites())]
		pub fn grant_invites(
			origin: OriginFor<T>,
			account: T::AccountId,
			count: u32,
		) -> DispatchResult {
			T::InvitationsOrigin::ensure_origin(origin)?;

			let mut available = AvailableInvites::<T>::get(&account);

			available = available.saturating_add(count);

			AvailableInvites::<T>::insert(&account, available);

			Self::deposit_event(Event::InvitesGranted { account, count });
			Ok(())
		}

		/// Remove all invites given to an account.
		///
		/// The origin must be `InvitationsOrigin`.
		///
		/// - `account`: The account to remove all invites from.
		/// - `limit`: The maximum number of pending invites to remove.
		#[pallet::call_index(16)]
		#[pallet::weight(T::WeightInfo::remove_available_and_pending_invites(*limit))]
		pub fn remove_available_and_pending_invites(
			origin: OriginFor<T>,
			account: T::AccountId,
			limit: u32,
		) -> DispatchResult {
			T::InvitationsOrigin::ensure_origin(origin)?;

			AvailableInvites::<T>::remove(&account);
			let res = PendingInvites::<T>::clear_prefix(&account, limit, None);
			if res.maybe_cursor.is_some() {
				Self::deposit_event(Event::SomeInvitesRemoved { inviter: account });
			} else {
				Self::deposit_event(Event::AllInvitesRemoved { inviter: account });
			}

			Ok(())
		}

		/// Invite an account.
		///
		/// The origin must be signed by an account and have some invites left.
		///
		/// - `ticket`: The invite ticket to set.
		#[pallet::call_index(17)]
		#[pallet::weight(T::WeightInfo::set_invite_ticket())]
		pub fn set_invite_ticket(origin: OriginFor<T>, ticket: TicketOf<T>) -> DispatchResult {
			let inviter = ensure_signed(origin)?;

			let mut available = AvailableInvites::<T>::get(&inviter);
			available = available.checked_sub(1).ok_or(Error::<T>::NoInvites)?;

			ensure!(
				!PendingInvites::<T>::contains_key(&inviter, &ticket),
				Error::<T>::AlreadyInvited
			);

			PendingInvites::<T>::insert(&inviter, &ticket, ());
			if available > 0 {
				AvailableInvites::<T>::insert(&inviter, available);
			} else {
				AvailableInvites::<T>::remove(&inviter);
			}

			Self::deposit_event(Event::InviteTicketSet { inviter, ticket });
			Ok(())
		}

		/// Cancel an invitation.
		///
		/// The origin must be signed by the account that owns the ticket to cancel.
		///
		/// - `ticket`: The invite ticket to cancel.
		#[pallet::call_index(18)]
		#[pallet::weight(T::WeightInfo::cancel_invite_ticket())]
		pub fn cancel_invite_ticket(origin: OriginFor<T>, ticket: TicketOf<T>) -> DispatchResult {
			let inviter = ensure_signed(origin)?;

			PendingInvites::<T>::take(&inviter, &ticket).ok_or(Error::<T>::NoTicket)?;

			let mut available = AvailableInvites::<T>::get(&inviter);
			available = available.saturating_add(1);
			AvailableInvites::<T>::insert(&inviter, available);
			Self::deposit_event(Event::InviteTicketCancelled { inviter, ticket });
			Ok(())
		}

		/// Set the configuration record. Must have privileged access to do this.
		#[pallet::weight(T::WeightInfo::set_configuration())]
		#[pallet::call_index(50)]
		pub fn set_configuration(
			origin: OriginFor<T>,
			config: ConfigRecord<BlockNumberFor<T>>,
		) -> DispatchResultWithPostInfo {
			T::ManagerOrigin::ensure_origin_or_root(origin)?;
			Configuration::<T>::put(&config);
			Self::deposit_event(Event::ConfigurationSet { config });
			Ok(Pays::No.into())
		}

		/// Set the values of the reimbursement awarded to referred candidates as well as their
		/// referrers, along with the number of reimbursements for each value. These values are
		/// stored in
		/// reverse order of their priority, so the last value in the list will be the first one
		/// to be used by the reimbursement system. The length of the two lists must be equal.
		///
		/// WARNING!
		///
		/// After the number of reimbursements is exhausted for a value in the list, the pallet
		/// starts using the next value immediately for future transfers.
		#[pallet::weight(T::WeightInfo::set_reimbursement_values(referred_values.len() as u32))]
		#[pallet::call_index(51)]
		pub fn set_reimbursement_values(
			origin: OriginFor<T>,
			referred_values: BoundedVec<(BalanceOf<T>, u32), T::MaxReimbursementValues>,
			referrer_values: BoundedVec<(BalanceOf<T>, u32), T::MaxReimbursementValues>,
		) -> DispatchResultWithPostInfo {
			T::ManagerOrigin::ensure_origin_or_root(origin)?;
			ensure!(
				referred_values.len() == referrer_values.len(),
				Error::<T>::InvalidReimbursementValues
			);
			let valid = referred_values
				.iter()
				.copied()
				.all(|(value, count)| value > BalanceOf::<T>::zero() && count > 0);
			ensure!(valid, Error::<T>::InvalidReimbursementValues);

			let valid = referrer_values
				.iter()
				.copied()
				.all(|(value, count)| value > BalanceOf::<T>::zero() && count > 0);
			ensure!(valid, Error::<T>::InvalidReimbursementValues);

			ReferredReimbursementValues::<T>::put(referred_values);
			ReferrerReimbursementValues::<T>::put(referrer_values);
			Ok(Pays::No.into())
		}
	}

	#[pallet::extra_constants]
	impl<T: Config> Pallet<T> {
		/// The account ID of the pot used for reward transfers.
		pub fn proof_of_ink_pot_id() -> T::AccountId {
			T::PotId::get().into_account_truncating()
		}
	}

	impl<T: Config> Pallet<T> {
		pub(super) fn referral_ok(referrer: PersonalId, target: &T::AccountId) {
			People::<T>::mutate_extant(referrer, |person| {
				person.active_referrals.retain(|x| x != target);
				person.successful_referrals.saturating_inc();
				person.pending_referral_rewards.saturating_inc();
				if person.allowed_referral_tickets < T::MaxActiveReferrals::get() {
					person.allowed_referral_tickets.saturating_inc();
				}
			});
			// We don't decrease the sufficient provider for the target yet.
			// It will be done when the candidate registers its key.
		}

		pub(super) fn referral_flaked(referrer: PersonalId, target: &T::AccountId) {
			People::<T>::mutate_extant(referrer, |person| {
				person.active_referrals.retain(|x| x != target);
			});
			let _ = frame_system::Pallet::<T>::dec_sufficients(target);
		}

		pub(super) fn referral_bad(referrer: PersonalId, target: &T::AccountId) {
			People::<T>::mutate_extant(referrer, |person| {
				person.active_referrals.retain(|x| x != target);
				person.bad_referrals.saturating_inc();
			});
			let _ = frame_system::Pallet::<T>::dec_sufficients(target);
		}

		pub(super) fn referral_contemptuous(referrer: PersonalId, target: &T::AccountId) {
			People::<T>::mutate_extant(referrer, |person| {
				person.active_referrals.retain(|x| x != target);
				person.bad_referrals.saturating_inc();
				person.banned = true;
			});
			let _ = frame_system::Pallet::<T>::dec_sufficients(target);
		}

		pub(super) fn bake_design(
			choice: InkChoice,
			entropy: Entropy,
			account: T::AccountId,
			id: PersonalId,
		) -> Result<InkSpec, Error<T>> {
			Ok(match choice {
				InkChoice::DesignedElective(family_id, design_index) => {
					ensure!(
						!CommittedDesigns::<T>::contains_key(family_id, design_index),
						Error::<T>::DesignTaken
					);
					let family =
						DesignFamilies::<T>::get(family_id).ok_or(Error::<T>::BadFamily)?;
					let FamilyKind::Designed { count } = family.kind else {
						Err(Error::<T>::WrongFamily)?
					};
					ensure!(design_index < count, Error::<T>::IndexTooBig);
					InkSpec::DesignedElective(family_id, design_index)
				},
				InkChoice::ProceduralAccount(family_id) => {
					let family =
						DesignFamilies::<T>::get(family_id).ok_or(Error::<T>::BadFamily)?;
					let FamilyKind::ProceduralAccount = family.kind else {
						Err(Error::<T>::WrongFamily)?
					};
					let a32 = account.using_encoded(|d| {
						// TODO: https://github.com/paritytech/individuality/issues/227
						// We may want to bound an explicit conversion to `[u8; 32]` for the type
						// `AccountId` such as `AsRef<[u8; 32]>`.
						// Currently we simply take the first 32 bytes of the account id.
						let mut buf: [u8; 32] = Default::default();
						let upper_bound = core::cmp::min(d.len(), buf.len());
						buf[..upper_bound].copy_from_slice(&d[..upper_bound]);
						buf
					});
					InkSpec::ProceduralAccount(family_id, a32)
				},
				InkChoice::ProceduralPersonal(family_id) => {
					let family =
						DesignFamilies::<T>::get(family_id).ok_or(Error::<T>::BadFamily)?;
					let FamilyKind::ProceduralPersonal = family.kind else {
						Err(Error::<T>::WrongFamily)?
					};
					InkSpec::ProceduralPersonal(family_id, id)
				},
				InkChoice::Procedural(family_id, variant) => {
					let family =
						DesignFamilies::<T>::get(family_id).ok_or(Error::<T>::BadFamily)?;
					let FamilyKind::Procedural { range } = family.kind else {
						Err(Error::<T>::WrongFamily)?
					};
					ensure!(variant < range, Error::<T>::IndexTooBig);
					InkSpec::Procedural(family_id, Self::entropy_to_seed(entropy, variant))
				},
				InkChoice::ProceduralDerivative(parent, maybe_secondary) => {
					let parent_design = People::<T>::get(parent)
						.ok_or(Error::<T>::BadParent)?
						.design
						.ok_or(Error::<T>::BadParent)?;
					let InkSpec::Procedural(family_id, mut seed) = parent_design else {
						Err(Error::<T>::DesignInvalid)?
					};
					if let Some(parent2) = maybe_secondary {
						let design2 = People::<T>::get(parent2)
							.ok_or(Error::<T>::BadParent)?
							.design
							.ok_or(Error::<T>::BadParent)?;
						let InkSpec::Procedural(_, seed2) = design2 else {
							Err(Error::<T>::DesignInvalid)?
						};
						seed[2] = seed2[2];
						seed[3] = seed2[3];
					}
					InkSpec::Procedural(family_id, Self::mutate_seed(seed, entropy))
				},
			})
		}

		fn mutate_seed(mut seed: ProceduralSeed, entropy: Entropy) -> ProceduralSeed {
			let mut index = [0usize; 3];
			for i in 0..index.len() {
				let range = 32 - i;
				let mut p = (entropy[i] as usize) % range;
				if index[0..i].contains(&p) {
					p = range;
				}
				index[i] = p;
				seed[p / 8] ^= 1 << (p % 8);
			}
			seed
		}

		fn entropy_to_seed(entropy: Entropy, variant: VariantIndex) -> ProceduralSeed {
			let mut seed = [0u8; 4];
			let mut id = variant as usize;
			if id < 8 {
				seed.copy_from_slice(&entropy[id * 4..id * 4 + 4])
			} else {
				let mut index = [0; 4];
				for i in 0..4 {
					let range = 32 - i;
					index[i] = id % range;
					for o in 0..i {
						if index[i] >= index[o] {
							index[i] += 1;
						}
					}
					seed[i] = entropy[index[i]];
					id /= range;
				}
			}
			seed
		}

		/// Register a new proven candidate.
		///
		/// # Warning: The storage must be rolled back when an error is returned.
		fn register_proven_candidate(
			account: T::AccountId,
			id: PersonalId,
			key: MemberOf<T>,
			design: InkSpec,
			referral_reward: ReferralReward<T::AccountId>,
			referrer_reward_kind: RewardKind,
			proof_of_ownership: <T::Crypto as GenerateVerifiable>::Signature,
		) -> DispatchResult {
			let msg =
				account.using_encoded(|bytes| [&PROOF_OF_OWNERSHIP_PREFIX[..], bytes].concat());
			ensure!(
				T::Crypto::verify_signature(&proof_of_ownership, &msg[..], &key),
				Error::<T>::InvalidProofOfOwnership
			);

			Candidates::<T>::remove(&account);
			People::<T>::insert(
				id,
				Person {
					design: Some(design),
					allowed_referral_tickets: 1,
					active_referrals: Default::default(),
					bad_referrals: 0,
					successful_referrals: 0,
					referrals: 0,
					derivatives: 0,
					banned: false,
					pending_referral_rewards: 0,
				},
			);

			T::People::recognize_personhood(id, Some(key))?;

			let ReferralReward::Destination(destination) = referral_reward;
			if referrer_reward_kind == RewardKind::Enabled {
				Self::register_referrer_reward(destination.clone())?;
			}
			Self::register_referred_reward(destination)?;

			Self::deposit_event(Event::PersonRegistered { account_id: account, personal_id: id });
			Ok(())
		}

		pub(crate) fn apply_footprint() -> Footprint {
			Footprint::from_mel::<(T::AccountId, CandidateOf<T>)>()
		}

		pub(crate) fn register_referred_reward(destination: T::AccountId) -> DispatchResult {
			let pot_id = Self::proof_of_ink_pot_id();

			let mut reward_values = ReferredReimbursementValues::<T>::get().unwrap_or_default();
			let (current_value, mut count) = match reward_values.pop() {
				Some((_, 0)) | None => {
					log::warn!(
						target: LOG_TARGET,
						"Referred reward requested without configured values",
					);
					return Ok(());
				},
				Some((current_value, count)) => (current_value, count),
			};

			let transferred = T::Currency::transfer(
				&pot_id,
				&destination,
				current_value,
				Preservation::Expendable,
			)?;
			debug_assert_eq!(transferred, current_value);

			count.saturating_dec();
			if count > 0 {
				let _ = reward_values
					.try_push((current_value, count))
					.defensive_proof("Push must work because we previously popped");
			}
			ReferredReimbursementValues::<T>::put(reward_values);
			Ok(())
		}

		pub(crate) fn register_referrer_reward(destination: T::AccountId) -> DispatchResult {
			let pot_id = Self::proof_of_ink_pot_id();

			let mut reward_values = ReferrerReimbursementValues::<T>::get().unwrap_or_default();
			let (current_value, mut count) = match reward_values.pop() {
				Some((_, 0)) | None => {
					log::warn!(
						target: LOG_TARGET,
						"Referrer reward requested without configured values",
					);
					return Ok(());
				},
				Some((current_value, count)) => (current_value, count),
			};

			let transferred = T::Currency::transfer(
				&pot_id,
				&destination,
				current_value,
				Preservation::Expendable,
			)?;
			debug_assert_eq!(transferred, current_value);

			count.saturating_dec();
			if count > 0 {
				let _ = reward_values
					.try_push((current_value, count))
					.defensive_proof("Push must work because we previously popped");
			}
			ReferrerReimbursementValues::<T>::put(reward_values);
			Ok(())
		}
	}

	/// Ensure that the origin `o` represents a signed origin or a `ReferredCandidate` origin.
	/// Returns `Ok` with the account that signed the extrinsic or the referred candidate or
	/// `BadOrigin` error.
	fn ensure_signed_or_referred<T>(o: T::RuntimeOrigin) -> Result<T::AccountId, BadOrigin>
	where
		T: Config,
	{
		if let Some(signer) = o.as_signer() {
			Ok(signer.clone())
		} else if let Ok(Origin::ReferredCandidate(referred)) = o.into() {
			Ok(referred)
		} else {
			Err(BadOrigin)
		}
	}

	/// Ensure that the origin `o` represents a signed origin, a `ReferredCandidate` origin or an
	/// `InvitedCandidate` origin. Returns `Ok` with the account that signed the extrinsic or the
	/// referred candidate or the invited candidate or `BadOrigin` error respectively.
	fn ensure_credible<T>(o: T::RuntimeOrigin) -> Result<T::AccountId, BadOrigin>
	where
		T: Config,
	{
		if let Some(signer) = o.as_signer() {
			Ok(signer.clone())
		} else if let Ok(Origin::ReferredCandidate(referred)) = o.clone().into() {
			Ok(referred)
		} else if let Ok(Origin::InvitedCandidate(invitee)) = o.into() {
			Ok(invitee)
		} else {
			Err(BadOrigin)
		}
	}

	/// Ensure that the origin `o` represents a signed origin or an
	/// `InvitedCandidate` origin. Returns `Ok` with the account that signed the extrinsic
	/// or the invited candidate or `BadOrigin` error respectively.
	fn ensure_signed_or_invited<T>(o: T::RuntimeOrigin) -> Result<T::AccountId, BadOrigin>
	where
		T: Config,
	{
		if let Some(signer) = o.as_signer() {
			Ok(signer.clone())
		} else if let Ok(Origin::InvitedCandidate(invitee)) = o.into() {
			Ok(invitee)
		} else {
			Err(BadOrigin)
		}
	}
}
