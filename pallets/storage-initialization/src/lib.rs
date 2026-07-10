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

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
mod weights;

use codec::Encode;
use cumulus_primitives_core::ParaId;
use frame_support::{
	migrations::{MigrationId, SteppedMigration, SteppedMigrationError},
	pallet_prelude::*,
	storage::with_transaction,
	traits::{
		fungibles::{Create, Inspect as FungiblesInspect, Mutate as FungiblesMutate},
		tokens::Preservation,
		ConstU32, Hooks,
	},
	weights::WeightMeter,
};
use frame_system::{pallet_prelude::BlockNumberFor, RawOrigin};
use indiv_pallet_game::{GameScheduleOf, WeightInfo as _};
use indiv_pallet_people::{MemberOf, WeightInfo as _};
#[cfg(feature = "try-runtime")]
use indiv_pallet_proof_of_ink::ConfigRecord;
#[cfg(feature = "try-runtime")]
use indiv_support::traits::PersonalId;
pub use pallet::*;
use sp_core::Get;
use sp_runtime::{
	traits::{SaturatedConversion, Saturating},
	TransactionOutcome,
};
use sp_std::{vec, vec::Vec};
pub use weights::WeightInfo;
use xcm::prelude::*;

pub trait ReserveSetter<AccountId, AssetId, ReserveData> {
	fn set_reserves(
		owner: &AccountId,
		asset_id: AssetId,
		reserves: BoundedVec<ReserveData, ConstU32<{ pallet_assets::MAX_RESERVES }>>,
	) -> DispatchResult;
}

impl<T, I> ReserveSetter<T::AccountId, T::AssetId, T::ReserveData> for pallet_assets::Pallet<T, I>
where
	T: pallet_assets::Config<I>,
	I: 'static,
{
	fn set_reserves(
		owner: &T::AccountId,
		asset_id: T::AssetId,
		reserves: BoundedVec<T::ReserveData, ConstU32<{ pallet_assets::MAX_RESERVES }>>,
	) -> DispatchResult {
		pallet_assets::Pallet::<T, I>::set_reserves(
			RawOrigin::Signed(owner.clone()).into(),
			asset_id.into(),
			reserves,
		)
	}
}

/// Simulate successful XCM transfer by directly funding the destination account.
/// This helper function can be used in tests and benchmarks to simulate the outcome
/// of a successful XCM asset transfer without actually executing XCM.
#[cfg(any(test, feature = "runtime-benchmarks"))]
pub fn simulate_xcm_transfer_success<T>() -> Result<(), sp_runtime::DispatchError>
where
	T: pallet::Config,
{
	use frame_support::traits::fungibles::Mutate;

	let asset_id: <T::Assets as FungiblesInspect<T::AccountId>>::AssetId =
		T::TransferAssetForeignId::get().into();
	let dest_account = T::AssetsDestAccount::get();
	let transfer_amount = T::AssetHubTransferAmount::get();

	<T as pallet::Config>::Assets::mint_into(
		asset_id,
		&dest_account,
		transfer_amount.saturated_into(),
	)
	.map(|_| ())
}

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config:
		frame_system::Config
		+ indiv_pallet_people::Config
		+ indiv_pallet_proof_of_ink::Config
		+ indiv_pallet_game::Config
		+ indiv_pallet_mob_rule::Config
		+ indiv_pallet_score::Config
		+ indiv_pallet_people_lite::Config
		+ indiv_pallet_members::Config
	{
		/// Weight information for the extrinsics in the pallet.
		type WeightInfo: WeightInfo;

		/// Assets interface for checking asset balances and transfers.
		type Assets: FungiblesInspect<
				Self::AccountId,
				AssetId: From<Location> + Parameter + Clone + MaxEncodedLen,
			> + FungiblesMutate<Self::AccountId>
			+ Create<Self::AccountId>;

		type ReserveData: From<(Location, bool)> + Clone;

		type ReserveSetter: ReserveSetter<
			Self::AccountId,
			<Self::Assets as FungiblesInspect<Self::AccountId>>::AssetId,
			Self::ReserveData,
		>;

		/// The account that receives invites in proof-of-ink and game pallets.
		type InvitesRecipient: Get<Self::AccountId>;

		/// The account that will receive the XCM transfer of assets.
		type AssetsDestAccount: Get<Self::AccountId>;

		/// Amount to transfer from Asset Hub.
		type AssetHubTransferAmount: Get<
			<Self::Assets as FungiblesInspect<Self::AccountId>>::Balance,
		>;

		/// XCM execution timeout in blocks.
		type XcmTimeout: Get<BlockNumberFor<Self>>;

		/// XCM sender for making cross-chain transfers.
		type XcmSender: SendXcm;

		/// This parachain ID.
		type ParachainInfo: sp_core::Get<ParaId>;

		/// Location of the asset on Asset Hub from the perspective of the People chain.
		/// The local Asset Hub perspective will be calculated automatically using relative_to.
		/// Format: Location::new(1, [Parachain(1000), PalletInstance(50), GeneralIndex(1)])
		type TransferAssetForeignId: Get<Location>;
	}

	/// Current state of the initialization process in on_poll hook.
	#[pallet::storage]
	pub type OnPollStatus<T> = StorageValue<_, OnPollState, ValueQuery>;

	/// Block number when XCM transfer was initiated.
	/// Used to track transfer timeout.
	#[pallet::storage]
	pub type XcmTransferInitiatedAt<T: Config> = StorageValue<_, BlockNumberFor<T>, OptionQuery>;

	#[derive(Encode, Decode, Clone, PartialEq, Eq, MaxEncodedLen, TypeInfo, Debug)]
	pub enum OnPollState {
		Inactive,
		CreatingAsset,
		XcmFundsTransfer,
		VerifyingFunds,
		FundingPots,
		SettingPeopleLiteAttestationAllowances,
		SchedulingMobRulePayouts,
		SchedulingScorePayouts,
		Done,
	}

	impl Default for OnPollState {
		fn default() -> Self {
			Self::Inactive
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// The foreign asset was created and reserves were set.
		AssetCreated,
		/// An XCM funds transfer was sent to Asset Hub.
		XcmFundsTransferSent,
		/// The XCM transfer timed out and will be retried.
		XcmFundsTransferTimedOut,
		/// Transferred funds have been verified.
		FundsVerified,
		/// All pots have been funded.
		PotsFunded,
		/// People Lite attestation allowances have been set.
		PeopleLiteAttestationAllowancesSet,
		/// Mob Rule payout schedule has been set.
		MobRulePayoutsScheduled,
		/// Score payout schedule has been set.
		ScorePayoutsScheduled,
		/// The one-time on_poll initialization has completed.
		OnPollInitializationCompleted,
		/// Migration: initial people have been recognized.
		MigrationPeopleRecognized,
		/// Migration: onboarding size has been set.
		MigrationOnboardingSizeSet,
		/// Migration: Proof-of-Ink pallet has been initialized.
		MigrationProofOfInkInitialized,
		/// Migration: games have been scheduled.
		MigrationGamesScheduled,
		/// Migration: invites have been granted.
		MigrationInvitesGranted,
		/// Migration: Proof-of-Ink reimbursement values have been set.
		MigrationReimbursementValuesSet,
		/// Migration: People Lite attestation allowances have been set.
		MigrationAttestationAllowancesSet,
		/// Migration has completed and on_poll initialization has been triggered.
		MigrationCompleted,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T>
	where
		T::AccountId: From<sp_runtime::AccountId32>,
	{
		fn on_poll(_n: BlockNumberFor<T>, weight: &mut frame_support::weights::WeightMeter) {
			// Use at most 50% of available weight to be cautious about weight
			// underestimates.
			let budget = weight.remaining() / 2;
			let mut meter = WeightMeter::with_limit(budget);
			Self::do_on_poll(&mut meter);
			weight.consume(meter.consumed());
		}

		fn integrity_test() {
			let transfer_amount = T::AssetHubTransferAmount::get();
			let pot_funding_config = get_pot_funding_configuration::<T>();

			let total_pot_funding = pot_funding_config.iter().fold(
				<T::Assets as FungiblesInspect<T::AccountId>>::Balance::default(),
				|acc, (_, amount)| acc + *amount,
			);

			assert!(
				transfer_amount >= total_pot_funding,
				"Asset transfer amount {transfer_amount:?} is insufficient to fund all pots (required: {total_pot_funding:?})"
			);
		}
	}

	impl<T: Config> Pallet<T>
	where
		T::AccountId: From<sp_runtime::AccountId32>,
	{
		fn do_on_poll(weight: &mut frame_support::weights::WeightMeter) {
			if weight.try_consume(<T as Config>::WeightInfo::on_poll_status_read()).is_err() {
				return
			};

			let current_state = OnPollStatus::<T>::get();

			match current_state {
				OnPollState::Inactive => {},
				OnPollState::CreatingAsset => {
					log::info!("Individuality pallets initialization - creating foreign asset");

					let create_weight = <T as Config>::WeightInfo::on_poll_create_asset();
					if weight.try_consume(create_weight).is_err() {
						return;
					}

					let (asset_id, owner, is_sufficient, min_balance) =
						get_transfer_asset_configuration::<T>();

					let mut asset_created = false;
					match <T as Config>::Assets::create(
						asset_id.clone(),
						owner.clone(),
						is_sufficient,
						min_balance,
					) {
						Ok(_) => {
							asset_created = true;
							log::info!("Individuality pallets initialization - foreign asset created successfully");
						},
						Err(e) => {
							log::error!("Individuality pallets initialization - foreign asset creation failed: {e:?}");
						},
					}

					if !asset_created && !<T as Config>::Assets::asset_exists(asset_id.clone()) {
						return;
					}

					match set_transfer_asset_reserves::<T>(&asset_id, &owner) {
						Ok(()) => {
							OnPollStatus::<T>::set(OnPollState::XcmFundsTransfer);
							Self::deposit_event(Event::<T>::AssetCreated);
						},
						Err(e) => {
							log::error!("Individuality pallets initialization - setting asset reserves failed: {e:?}");
						},
					}
				},
				OnPollState::XcmFundsTransfer => {
					log::info!(
						"Individuality pallets initialization - executing XCM funds transfer"
					);

					let xcm_weight = <T as Config>::WeightInfo::on_poll_xcm_transfer();
					if !weight.can_consume(xcm_weight) {
						return;
					}

					let current_block = frame_system::Pallet::<T>::block_number();

					match Self::execute_xcm_transfer() {
						Ok(_) => {
							log::info!("Individuality pallets initialization - XCM funds transfer sent successfully");
							XcmTransferInitiatedAt::<T>::put(current_block);
							OnPollStatus::<T>::set(OnPollState::VerifyingFunds);
							Self::deposit_event(Event::<T>::XcmFundsTransferSent);
						},
						Err(e) => {
							log::error!("Individuality pallets initialization - XCM funds transfer failed: {e:?}");
						},
					}

					weight.consume(xcm_weight);
				},
				OnPollState::VerifyingFunds => {
					log::info!(
						"Individuality pallets initialization - checking transferred funds arrival"
					);

					let verify_weight = <T as Config>::WeightInfo::on_poll_verify_funds();
					if !weight.can_consume(verify_weight) {
						return;
					}

					let current_block = frame_system::Pallet::<T>::block_number();
					let people_chain_account = T::AssetsDestAccount::get();
					let expected_amount = T::AssetHubTransferAmount::get();

					let transfer_asset_id: <T::Assets as FungiblesInspect<T::AccountId>>::AssetId =
						T::TransferAssetForeignId::get().into();
					let current_balance =
						<T as Config>::Assets::balance(transfer_asset_id, &people_chain_account);

					if current_balance >= expected_amount {
						log::info!("Individuality pallets initialization - transferred funds verified, proceeding to pot funding");
						OnPollStatus::<T>::set(OnPollState::FundingPots);
						XcmTransferInitiatedAt::<T>::kill();
						Self::deposit_event(Event::<T>::FundsVerified);
					} else if let Some(initiated_at) = XcmTransferInitiatedAt::<T>::get() {
						let timeout = T::XcmTimeout::get();
						if current_block.saturating_sub(initiated_at) > timeout {
							log::error!(
										"Individuality pallets initialization - XCM transfer timeout reached, retrying"
									);
							XcmTransferInitiatedAt::<T>::kill();
							OnPollStatus::<T>::set(OnPollState::XcmFundsTransfer);
							Self::deposit_event(Event::<T>::XcmFundsTransferTimedOut);
						} else {
							log::debug!(
								"Individuality pallets initialization - still waiting for XCM transfer. Current balance: {current_balance:?}, Expected: {expected_amount:?}"
										);
						}
					} else {
						log::error!("Individuality pallets initialization - XCM transfer not initiated, resetting to transfer state");
						OnPollStatus::<T>::set(OnPollState::XcmFundsTransfer);
					}

					weight.consume(verify_weight);
				},
				OnPollState::FundingPots => {
					log::info!("Individuality pallets initialization - funding pots");

					let fund_weight = <T as Config>::WeightInfo::fund_pots();
					if !weight.can_consume(fund_weight) {
						return;
					}

					let pot_funding_config = get_pot_funding_configuration::<T>();
					let assets_dest_account = T::AssetsDestAccount::get();
					let transfer_asset_id: <T::Assets as FungiblesInspect<T::AccountId>>::AssetId =
						T::TransferAssetForeignId::get().into();

					let mut funding_error = None;
					for (pot_account, amount) in pot_funding_config {
						match <T as Config>::Assets::transfer(
							transfer_asset_id.clone(),
							&assets_dest_account,
							&pot_account,
							amount,
							Preservation::Expendable,
						) {
							Ok(_) => {
								log::debug!(
									"Successfully funded pot account with amount: {amount:?}"
								);
							},
							Err(e) => {
								funding_error = Some(e);
								break;
							},
						}
					}

					if funding_error.is_none() {
						log::info!(
							"Individuality pallets initialization - pots funded successfully"
						);
						OnPollStatus::<T>::set(OnPollState::SettingPeopleLiteAttestationAllowances);
						Self::deposit_event(Event::<T>::PotsFunded);
					} else {
						log::error!(
								"Individuality pallets initialization - pot funding failed: {funding_error:?}"
								);
						// The state kept to retry on the next on_poll trigger
					}
					weight.consume(fund_weight);
				},
				OnPollState::SettingPeopleLiteAttestationAllowances => {
					let step_weight =
						<T as Config>::WeightInfo::on_poll_set_people_lite_attestation_allowances();
					if !weight.can_consume(step_weight) {
						return;
					}
					let allowances = get_people_lite_attestation_allowances::<T>();

					log::info!(
						"Individuality pallets initialization - setting People Lite attestation allowances"
					);

					let mut step_error = None;
					// The `step_weight` already includes all the iterations, thats why don't
					// consume it per `allowance`
					for (account, count) in allowances {
						// Idempotency guard: skip accounts that already have a non-zero
						// allowance.
						if !indiv_pallet_people_lite::AttestationAllowance::<T>::get(&account)
							.is_zero()
						{
							continue;
						}
						if let Err(e) =
							indiv_pallet_people_lite::Pallet::<T>::increase_attestation_allowance(
								RawOrigin::Root.into(),
								account,
								count,
							) {
							step_error = Some(e);
							break;
						}
					}

					if step_error.is_none() {
						OnPollStatus::<T>::set(OnPollState::SchedulingMobRulePayouts);
						Self::deposit_event(Event::<T>::PeopleLiteAttestationAllowancesSet);
					} else {
						log::error!(
							"Individuality pallets initialization - failed to set People Lite attestation allowances: {step_error:?}"
						);
						// State kept to retry on the next on_poll trigger.
					}
					weight.consume(step_weight);
				},
				OnPollState::SchedulingMobRulePayouts => {
					log::info!(
						"Individuality pallets initialization - scheduling Mob Rule payouts"
					);

					let schedule_weight =
						<T as Config>::WeightInfo::on_poll_schedule_mob_rule_payouts();
					if !weight.can_consume(schedule_weight) {
						return;
					}

					let (amount_per_round, count, period) =
						get_initial_mob_rule_payout_rounds_config::<T>();
					let schedule = indiv_pallet_mob_rule::PayoutSchedule {
						remaining: count,
						amount_per_round: amount_per_round.into(),
						period,
					};

					let current_schedules = indiv_pallet_mob_rule::RoundSchedules::<T>::get();
					let mut new_schedules = current_schedules;
					match new_schedules.try_push(schedule.clone()) {
						Ok(_) => {
							indiv_pallet_mob_rule::RoundSchedules::<T>::set(new_schedules);
							log::info!(
								"Individuality pallets initialization - Mob Rule payouts scheduled"
							);
							OnPollStatus::<T>::set(OnPollState::SchedulingScorePayouts);
							Self::deposit_event(Event::<T>::MobRulePayoutsScheduled);
						},
						Err(_) => {
							log::error!(
										"Individuality pallets initialization - Failed to add mob rule payout schedule"
									);
							// State remains to retry
						},
					}

					weight.consume(schedule_weight);
				},
				OnPollState::SchedulingScorePayouts => {
					log::info!(
						"Individuality pallets initialization - scheduling Score pallet payouts"
					);

					let schedule_weight =
						<T as Config>::WeightInfo::on_poll_schedule_score_payouts();
					if !weight.can_consume(schedule_weight) {
						return;
					}

					let (amount_per_round, count, duration) =
						get_initial_score_payout_rounds_config::<T>();
					let schedule = indiv_pallet_score::PayoutSchedule {
						remaining: count,
						amount_per_round: amount_per_round.saturated_into(),
						duration,
					};

					let current_schedules = indiv_pallet_score::RoundSchedules::<T>::get();
					let mut new_schedules = current_schedules;
					match new_schedules.try_push(schedule.clone()) {
						Ok(_) => {
							indiv_pallet_score::RoundSchedules::<T>::set(new_schedules);
							log::info!(
								"Individuality pallets initialization - Score pallet payouts scheduled"
							);
							OnPollStatus::<T>::set(OnPollState::Done);
							log::info!(
								"Individuality pallets initialization - on_poll initialization completed"
							);
							Self::deposit_event(Event::<T>::ScorePayoutsScheduled);
							Self::deposit_event(Event::<T>::OnPollInitializationCompleted);
						},
						Err(_) => {
							log::error!(
										"Individuality pallets initialization - Failed to add score payout schedule"
									);
							// State remains to retry
						},
					}
					weight.consume(schedule_weight);
				},
				OnPollState::Done => {},
			}
		}
	}

	impl<T: Config> Pallet<T> {
		/// Convert foreign asset location to local Asset Hub perspective using relative_to.
		///
		/// Takes the foreign asset location (from People chain's perspective) and converts it
		/// to how Asset Hub would address the same asset locally.
		fn convert_foreign_to_local_asset_location() -> Result<(Location, Junctions), DispatchError>
		{
			let foreign_location = T::TransferAssetForeignId::get();

			if foreign_location.parent_count() != 1 {
				return Err(DispatchError::Other("Foreign location must have exactly 1 parent"));
			}

			let interior = foreign_location.interior();
			if interior.len() < 1 {
				return Err(DispatchError::Other(
					"Foreign location must have at least one junction",
				));
			}

			let asset_hub_para_id = match interior.first() {
				Some(Junction::Parachain(para_id)) => *para_id,
				Some(_) => return Err(DispatchError::Other("First junction must be Parachain")),
				None => return Err(DispatchError::Other("Foreign location has no junctions")),
			};

			let asset_hub_context = Junctions::from([Junction::Parachain(asset_hub_para_id)]);
			let local_location = interior.clone().relative_to(&asset_hub_context);

			// To validate result makes sense (should have 0 parents for local Asset Hub
			// perspective)
			if local_location.parent_count() != 0 {
				return Err(DispatchError::Other("Converted location should have 0 parents"));
			}

			Ok((local_location, asset_hub_context))
		}

		/// Execute XCM transfer by sending message to Asset Hub requesting assets.
		fn execute_xcm_transfer() -> Result<(), DispatchError> {
			let transfer_amount = T::AssetHubTransferAmount::get();
			let assets_dest_account = T::AssetsDestAccount::get();

			let (local_asset_location, asset_hub_context) =
				Self::convert_foreign_to_local_asset_location()?;
			let asset_hub_para_id = match asset_hub_context.first() {
				Some(Junction::Parachain(para_id)) => *para_id,
				_ => return Err(DispatchError::Other("Invalid foreign location format")),
			};

			let asset_hub_location = Location::new(1, [Parachain(asset_hub_para_id)]);

			let account_bytes = assets_dest_account.encode();
			let mut id = [0u8; 32];
			id[..account_bytes.len().min(32)]
				.copy_from_slice(&account_bytes[..account_bytes.len().min(32)]);
			let this_chain_location = Location::new(1, [Parachain(T::ParachainInfo::get().into())]);
			let beneficiary = Location::new(0, [AccountId32 { network: None, id }]);

			let transfer_asset = Asset {
				id: AssetId(local_asset_location),
				fun: Fungible(transfer_amount.saturated_into::<u128>()),
			};

			use frame_support::BoundedVec;
			use xcm::latest::{AssetFilter, AssetTransferFilter};

			let xcm_message = Xcm(vec![
				UnpaidExecution {
					weight_limit: WeightLimit::Unlimited,
					check_origin: Some(this_chain_location.clone().into()),
				},
				WithdrawAsset(vec![transfer_asset.clone()].into()),
				InitiateTransfer {
					destination: this_chain_location.clone(),
					remote_fees: None,
					preserve_origin: true,
					assets: BoundedVec::try_from(sp_std::vec![
						AssetTransferFilter::ReserveDeposit(AssetFilter::Definite(
							transfer_asset.clone().into()
						))
					])
					.unwrap_or_default(),
					remote_xcm: Xcm(vec![DepositAsset {
						assets: vec![Asset {
							id: AssetId(T::TransferAssetForeignId::get()),
							fun: Fungible(transfer_amount.saturated_into::<u128>()),
						}]
						.into(),
						beneficiary,
					}]),
				},
			]);

			send_xcm::<T::XcmSender>(
				asset_hub_location,
				xcm_message
					.try_into()
					.map_err(|_| DispatchError::Other("Invalid XCM message"))?,
			)
			.map(|_| ())
			.map_err(|e| {
				log::error!("Individuality pallets initialization - XCM send failed: {e:?}");
				DispatchError::Other("XCM send failed")
			})
		}
	}
}

pub const PALLET_ID: &[u8; 8] = b"pal-init";
pub const MIGRATION_ID: &[u8; 25] = b"pallet-individuality-init";

// Not used by any runtime.
// TODO: https://github.com/paritytech/individuality/issues/942.
#[derive(Encode, Decode, Clone, PartialEq, Eq, MaxEncodedLen)]
pub enum MigrationState {
	NotStarted,
	InitializingPeopleForPalletPeople,
	InitializingOnboardingSizeForPalletPeople,
	InitializingPalletProofOfInk,
	SchedulingGames,
	GrantingInvites,
	SettingProofOfInkReimbursementValues,
	SettingPeopleLiteAttestationAllowances,
	Finished,
}

pub struct InitializeIndividualityPallets<T>(PhantomData<T>);

impl<T: Config> SteppedMigration for InitializeIndividualityPallets<T>
where
	T::AccountId: From<sp_runtime::AccountId32>,
{
	type Cursor = MigrationState;
	type Identifier = MigrationId<25>;

	fn id() -> <InitializeIndividualityPallets<T> as SteppedMigration>::Identifier {
		MigrationId { pallet_id: *MIGRATION_ID, version_from: 0, version_to: 1 }
	}

	fn step(
		cursor: Option<<InitializeIndividualityPallets<T> as SteppedMigration>::Cursor>,
		meter: &mut WeightMeter,
	) -> Result<
		Option<<InitializeIndividualityPallets<T> as SteppedMigration>::Cursor>,
		SteppedMigrationError,
	> {
		let state = cursor.unwrap_or(MigrationState::NotStarted);

		match state {
			MigrationState::NotStarted => {
				log::info!("Individuality pallets initialization is about to start");

				Ok(Some(MigrationState::InitializingPeopleForPalletPeople))
			},

			MigrationState::InitializingPeopleForPalletPeople => {
				log::info!("Individuality pallets initialization - adding people to pallet people");

				let initial_people = get_initial_people_keys();
				let keys_count = initial_people.len();

				let weight =
					<T as indiv_pallet_people::Config>::WeightInfo::force_recognize_personhood(
						keys_count as u32,
					);
				if !meter.can_consume(weight) {
					return Ok(Some(MigrationState::InitializingPeopleForPalletPeople));
				}

				let keys: Vec<MemberOf<T>> = initial_people
					.into_iter()
					.filter_map(|raw_key| {
						use codec::Decode;
						match MemberOf::<T>::decode(&mut &raw_key[..]) {
							Ok(key) => Some(key),
							Err(err) => {
								log::error!(
									"Failed to decode initial people key: {err:?}, skipping this key"
								);
								None
							},
						}
					})
					.collect();

				let _ = with_transaction(|| {
					let res = indiv_pallet_people::Pallet::<T>::force_recognize_personhood(
						frame_system::RawOrigin::Root.into(),
						keys,
					);
					match res {
						Ok(_) => TransactionOutcome::Commit(Result::<_, DispatchError>::Ok(true)),
						Err(err) =>
							TransactionOutcome::Rollback(Result::<_, DispatchError>::Err(err.error)),
					}
				})
				.map_err(|err| {
					log::error!(
					"Individuality pallets initialization - failed to recognize personhood: {err:?}"
					)
				});

				meter.consume(weight);
				Pallet::<T>::deposit_event(Event::<T>::MigrationPeopleRecognized);
				Ok(Some(MigrationState::InitializingOnboardingSizeForPalletPeople))
			},

			MigrationState::InitializingOnboardingSizeForPalletPeople => {
				log::info!(
					"Individuality pallets initialization - setting OnboardingSize for pallet people"
				);

				let weight = <T as frame_system::Config>::DbWeight::get().writes(1);
				if !meter.can_consume(weight) {
					return Ok(Some(MigrationState::InitializingOnboardingSizeForPalletPeople));
				}

				let onboarding_size = get_initial_onboarding_size();
				indiv_pallet_members::OnboardingSize::<T>::insert(
					indiv_pallet_people::PEOPLE_MEMBER_IDENTIFIER,
					onboarding_size,
				);

				meter.consume(weight);
				Pallet::<T>::deposit_event(Event::<T>::MigrationOnboardingSizeSet);
				Ok(Some(MigrationState::InitializingPalletProofOfInk))
			},

			MigrationState::InitializingPalletProofOfInk => {
				log::info!(
					"Individuality pallets initialization - initializing pallet Proof-of-Ink"
				);

				let weight = <T as Config>::WeightInfo::init_pallet_proof_of_ink();
				if !meter.can_consume(weight) {
					return Ok(Some(MigrationState::InitializingPalletProofOfInk));
				}

				let design_families = get_proof_of_ink_design_families();
				let config = get_proof_of_ink_configuration::<T>();

				for (family_index, family_kind, family_id) in design_families {
					let family =
						indiv_pallet_proof_of_ink::Family { kind: family_kind, id: family_id };
					indiv_pallet_proof_of_ink::DesignFamilies::<T>::insert(family_index, family);
				}

				indiv_pallet_proof_of_ink::Configuration::<T>::put(config);

				meter.consume(weight);
				Pallet::<T>::deposit_event(Event::<T>::MigrationProofOfInkInitialized);
				Ok(Some(MigrationState::SchedulingGames))
			},

			MigrationState::SchedulingGames => {
				log::info!("Individuality pallets initialization - scheduling games");

				let game_schedules = get_game_schedules::<T>();
				// Use indiv_pallet_game weight function for proper weight calculation
				let weight = indiv_pallet_game::weights::SubstrateWeight::<T>::schedule_games(
					game_schedules.len() as u32,
				);
				if !meter.can_consume(weight) {
					return Ok(Some(MigrationState::SchedulingGames));
				}

				let _ = with_transaction(|| {
					let res = indiv_pallet_game::Pallet::<T>::schedule_games(
						frame_system::RawOrigin::Root.into(),
						game_schedules,
					);
					match res {
						Ok(_) => TransactionOutcome::Commit(Result::<_, DispatchError>::Ok(true)),
						Err(err) =>
							TransactionOutcome::Rollback(Result::<_, DispatchError>::Err(err)),
					}
				})
				.map_err(|err| {
					log::error!(
						"Individuality pallets initialization - failed to schedule games: {err:?}"
					)
				});

				meter.consume(weight);
				Pallet::<T>::deposit_event(Event::<T>::MigrationGamesScheduled);
				Ok(Some(MigrationState::GrantingInvites))
			},

			MigrationState::GrantingInvites => {
				log::info!("Individuality pallets initialization - granting invites");

				const INVITES_COUNT: u32 = 100;

				let weight = <T as pallet::Config>::WeightInfo::grant_invites_migration();
				if !meter.can_consume(weight) {
					return Ok(Some(MigrationState::GrantingInvites));
				}

				let invites_recipient = T::InvitesRecipient::get();

				with_transaction(|| {
					let poi_result = indiv_pallet_proof_of_ink::Pallet::<T>::grant_invites(
						RawOrigin::Root.into(),
						invites_recipient.clone(),
						INVITES_COUNT,
					);

					if let Err(e) = poi_result {
						log::error!("Failed to grant proof-of-ink invites: {e:?}");
						return TransactionOutcome::Rollback(Err(
							"Failed to grant proof-of-ink invites",
						));
					}

					let game_result = indiv_pallet_game::Pallet::<T>::grant_invites(
						RawOrigin::Root.into(),
						invites_recipient.clone(),
						INVITES_COUNT,
					);

					if let Err(e) = game_result {
						log::error!("Failed to grant game invites: {e:?}");
						return TransactionOutcome::Rollback(Err("Failed to grant game invites"));
					}

					TransactionOutcome::Commit(Ok(()))
				})
				.map_err(|_| SteppedMigrationError::Failed)?;

				log::info!("Individuality pallets initialization - invites granted");

				meter.consume(weight);
				Pallet::<T>::deposit_event(Event::<T>::MigrationInvitesGranted);
				Ok(Some(MigrationState::SettingProofOfInkReimbursementValues))
			},

			MigrationState::SettingProofOfInkReimbursementValues => {
				log::info!(
					"Individuality pallets initialization - setting Proof-of-Ink reimbursement values"
				);

				let weight =
					<T as pallet::Config>::WeightInfo::set_proof_of_ink_reimbursement_values();
				if !meter.can_consume(weight) {
					return Ok(Some(MigrationState::SettingProofOfInkReimbursementValues));
				}

				let (referred_values, referrer_values) = get_initial_reimbursement_values::<T>();

				match with_transaction(|| {
					let res = indiv_pallet_proof_of_ink::Pallet::<T>::set_reimbursement_values(
						RawOrigin::Root.into(),
						referred_values.clone(),
						referrer_values.clone(),
					);
					match res {
						Ok(_) => TransactionOutcome::Commit(Result::<_, DispatchError>::Ok(true)),
						Err(err) =>
							TransactionOutcome::Rollback(Result::<_, DispatchError>::Err(err.error)),
					}
				}) {
					Ok(_) => {
						log::info!(
							"Individuality pallets initialization - Proof-of-Ink reimbursement values set"
						);
					},
					Err(err) => {
						log::error!("Individuality pallets initialization - Failed to set Proof-of-Ink reimbursement values: {err:?}");
						// State moves on. No retry.
					},
				}

				meter.consume(weight);
				Pallet::<T>::deposit_event(Event::<T>::MigrationReimbursementValuesSet);
				Ok(Some(MigrationState::SettingPeopleLiteAttestationAllowances))
			},

			MigrationState::SettingPeopleLiteAttestationAllowances => {
				log::info!(
					"Individuality pallets initialization - setting People Lite attestation allowances"
				);

				let allowances = get_people_lite_attestation_allowances::<T>();
				let weight =
					<T as pallet::Config>::WeightInfo::set_people_lite_attestation_allowances(
						allowances.len() as u32,
					);
				if !meter.can_consume(weight) {
					return Ok(Some(MigrationState::SettingPeopleLiteAttestationAllowances));
				}

				match with_transaction(|| {
					for (account, count) in allowances {
						let res =
							indiv_pallet_people_lite::Pallet::<T>::increase_attestation_allowance(
								RawOrigin::Root.into(),
								account,
								count,
							);
						if let Err(e) = res {
							log::error!("Failed to set attestation allowance: {e:?}");
							return TransactionOutcome::Rollback(Result::<_, DispatchError>::Err(e));
						}
					}
					TransactionOutcome::Commit(Result::<_, DispatchError>::Ok(true))
				}) {
					Ok(_) => {
						log::info!(
							"Individuality pallets initialization - People Lite attestation allowances set"
						);
					},
					Err(err) => {
						defensive!(
							"Individuality pallets initialization - Failed to set People Lite attestation allowances",
							err
						);
						// State moves on. No retry.
					},
				}

				meter.consume(weight);
				Pallet::<T>::deposit_event(Event::<T>::MigrationAttestationAllowancesSet);
				Ok(Some(MigrationState::Finished))
			},

			MigrationState::Finished => {
				log::info!("Individuality pallets initialization completed");

				// Signal that on_poll should start
				OnPollStatus::<T>::set(OnPollState::CreatingAsset);

				Pallet::<T>::deposit_event(Event::<T>::MigrationCompleted);

				Ok(None)
			},
		}
	}

	/// Checks if all the storage items affected by this migration are empty,
	/// which means that no initialization was yet done,
	/// and that the provided initialization values are not empty.
	#[cfg(feature = "try-runtime")]
	fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::TryRuntimeError> {
		use codec::Encode;

		ensure!(
			indiv_pallet_people::People::<T>::iter().count() == 0,
			"People storage should be empty before migration"
		);

		ensure!(
			indiv_pallet_people::NextPersonalId::<T>::get() == 0,
			"NextPersonalId should be 0 before migration"
		);

		ensure!(
			indiv_pallet_members::OnboardingSize::<T>::get(
				indiv_pallet_people::PEOPLE_MEMBER_IDENTIFIER
			) == 0,
			"OnboardingSize should be 0 before migration"
		);

		ensure!(
			indiv_pallet_proof_of_ink::DesignFamilies::<T>::iter().count() == 0,
			"DesignFamilies in pallet proof of ink should be empty before migration"
		);

		ensure!(
			indiv_pallet_proof_of_ink::Configuration::<T>::get() == Default::default(),
			"Configuration in pallet proof of ink should be empty before migration"
		);

		let people = get_initial_people_keys();
		ensure!(people.len() != 0, "Number of people to inject should not be 0");

		let design_families = get_proof_of_ink_design_families();
		ensure!(design_families.len() != 0, "Number of design families to inject should not be 0");

		let configuration = get_proof_of_ink_configuration::<T>();
		ensure!(
			configuration != Default::default(),
			"Pallet proof of ink configuration to inject should not be empty"
		);

		// Verify game scheduling data is valid
		let game_schedules = get_game_schedules::<T>();
		ensure!(game_schedules.len() != 0, "Number of game schedules to inject should not be 0");

		// Verify game pallet storage is empty before migration
		ensure!(
			indiv_pallet_game::GameSchedules::<T>::get().len() == 0,
			"GameSchedules should be empty before migration"
		);

		// Verify invites storages are empty before migration
		let invite_recipient = T::InvitesRecipient::get();
		ensure!(
			indiv_pallet_proof_of_ink::AvailableInvites::<T>::get(&invite_recipient) == 0,
			"AvailableInvites in proof-of-ink should be empty before migration"
		);
		ensure!(
			indiv_pallet_game::AvailableInvites::<T>::get(&invite_recipient) == 0,
			"AvailableInvites in game pallet should be empty before migration"
		);

		// Verify reimbursement values are empty before migration
		ensure!(
			indiv_pallet_proof_of_ink::ReferredReimbursementValues::<T>::get().is_none(),
			"ReferredReimbursementValues should be None before migration"
		);
		ensure!(
			indiv_pallet_proof_of_ink::ReferrerReimbursementValues::<T>::get().is_none(),
			"ReferrerReimbursementValues should be None before migration"
		);

		// Verify people lite attestation allowances are empty before migration
		let allowances_config = get_people_lite_attestation_allowances::<T>();
		for (account, _) in &allowances_config {
			ensure!(
				indiv_pallet_people_lite::AttestationAllowance::<T>::get(account) == 0,
				"AttestationAllowance should be 0 before migration"
			);
		}
		ensure!(
			allowances_config.len() > 0,
			"Number of attestation allowances to set should not be 0"
		);

		let expected_onboarding_size = get_initial_onboarding_size();

		let expected_invites_count = 100u32;

		let (referred_vals, referrer_vals) = get_initial_reimbursement_values::<T>();
		let expected_referred_values = Some(referred_vals);
		let expected_referrer_values = Some(referrer_vals);

		let state = (
			people.len() as u32,
			expected_onboarding_size,
			design_families.len() as u32,
			configuration,
			game_schedules.len() as u32,
			expected_invites_count,
			expected_referred_values,
			expected_referrer_values,
			allowances_config,
		);

		Ok(state.encode())
	}

	/// Checks if storage items lengths or values are matching the desired ones.
	#[cfg(feature = "try-runtime")]
	fn post_upgrade(state: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
		use codec::Decode;

		type ReimbursementValues<T> = BoundedVec<
			(indiv_pallet_proof_of_ink::BalanceOf<T>, u32),
			<T as indiv_pallet_proof_of_ink::Config>::MaxReimbursementValues,
		>;

		let (
			expected_people_count,
			expected_onboarding_size,
			expected_design_families_count,
			expected_configuration,
			expected_game_schedules_count,
			expected_invites_count,
			expected_referred_values,
			expected_referrer_values,
			expected_allowances,
		): (
			u32,
			u32,
			u32,
			ConfigRecord<BlockNumberFor<T>>,
			u32,
			u32,
			Option<ReimbursementValues<T>>,
			Option<ReimbursementValues<T>>,
			Vec<(T::AccountId, u32)>,
		) = Decode::decode(&mut &state[..]).map_err(|_| "Failed to decode state")?;

		// --- Pallet People checks

		let actual_people_count = indiv_pallet_people::People::<T>::iter().count() as u32;
		ensure!(actual_people_count == expected_people_count, "People count mismatch");

		let keys_count = indiv_pallet_people::Keys::<T>::iter().count() as u32;
		ensure!(keys_count == expected_people_count, "Keys count mismatch");

		let (head, _) = indiv_pallet_members::QueuePageIndices::<T>::get(
			indiv_pallet_people::PEOPLE_MEMBER_IDENTIFIER,
		);
		let total_in_queue = indiv_pallet_members::OnboardingQueue::<T>::get(
			indiv_pallet_people::PEOPLE_MEMBER_IDENTIFIER,
			head,
		)
		.len() as u32;
		ensure!(total_in_queue == expected_people_count, "OnboardingQueue count mismatch");

		if expected_people_count > 0 {
			let next_id = indiv_pallet_people::NextPersonalId::<T>::get();
			ensure!(
				next_id >= expected_people_count as PersonalId,
				"NextPersonalId not set correctly"
			);
		}

		let actual_onboarding_size = indiv_pallet_members::OnboardingSize::<T>::get(
			indiv_pallet_people::PEOPLE_MEMBER_IDENTIFIER,
		);
		ensure!(
			actual_onboarding_size == expected_onboarding_size,
			"OnboardingSize not set correctly"
		);

		// --- Pallet Proof of Ink checks

		let actual_design_families_count =
			indiv_pallet_proof_of_ink::DesignFamilies::<T>::iter().count() as u32;
		ensure!(
			actual_design_families_count == expected_design_families_count,
			"Design families count mismatch"
		);

		let actual_configuration = indiv_pallet_proof_of_ink::Configuration::<T>::get();
		ensure!(actual_configuration == expected_configuration, "Configuration mismatch");

		// --- Game Pallet checks
		let actual_game_schedules = indiv_pallet_game::GameSchedules::<T>::get();
		ensure!(
			actual_game_schedules.len() as u32 == expected_game_schedules_count,
			"Game schedules count mismatch"
		);

		// --- Invites Granting checks
		let invites_recipient = T::InvitesRecipient::get();
		let actual_poi_invites =
			indiv_pallet_proof_of_ink::AvailableInvites::<T>::get(&invites_recipient);
		let actual_game_invites = indiv_pallet_game::AvailableInvites::<T>::get(&invites_recipient);

		ensure!(
			actual_poi_invites == expected_invites_count,
			"Proof-of-ink invites count mismatch",
		);

		ensure!(actual_game_invites == expected_invites_count, "Game invites count mismatch",);

		// --- Reimbursement Values checks
		let actual_referred_values =
			indiv_pallet_proof_of_ink::ReferredReimbursementValues::<T>::get();
		let actual_referrer_values =
			indiv_pallet_proof_of_ink::ReferrerReimbursementValues::<T>::get();

		match (expected_referred_values, expected_referrer_values) {
			(Some(exp_referred), Some(exp_referrer)) => {
				ensure!(
					actual_referred_values == Some(exp_referred),
					"ReferredReimbursementValues mismatch"
				);
				ensure!(
					actual_referrer_values == Some(exp_referrer),
					"ReferrerReimbursementValues mismatch"
				);
			},
			(None, None) => {
				ensure!(
					actual_referred_values.is_none(),
					"ReferredReimbursementValues should be None"
				);
				ensure!(
					actual_referrer_values.is_none(),
					"ReferrerReimbursementValues should be None"
				);
			},
			_ => {
				return Err("Inconsistent reimbursement values state".into());
			},
		}

		// --- People Lite Attestation Allowances checks
		for (account, expected_count) in expected_allowances {
			let actual_count = indiv_pallet_people_lite::AttestationAllowance::<T>::get(&account);
			ensure!(actual_count == expected_count, "AttestationAllowance mismatch for account");
		}

		Ok(())
	}
}

/// Keys of the initial set of people to be recognized during migration.
/// These are encoded Bandersnatch public keys.
fn get_initial_people_keys() -> Vec<Vec<u8>> {
	use hex_literal::hex;
	vec![
		hex!("5e465beb01dbafe160ce8216047f2155dd0569f058afd52dcea601025a8d161d").to_vec(),
		hex!("3f9828dfc4e8fdc12ed5f7e6f380b5d30ff328613e6d3e4a27e24ca3e64474b9").to_vec(),
		hex!("7539d69a545904e39cabc487a63ae3b9f863afefede3f49c786a5047c5189cdb").to_vec(),
	]
}

/// Pot funding configuration for initialization.
/// Returns a vector of (pot_account, funding_amount) tuples.
fn get_pot_funding_configuration<T: Config>(
) -> Vec<(T::AccountId, <T::Assets as FungiblesInspect<T::AccountId>>::Balance)> {
	let mut funding_config = Vec::new();

	// Proof of Ink pot
	let proof_of_ink_pot = indiv_pallet_proof_of_ink::Pallet::<T>::proof_of_ink_pot_id();
	funding_config.push((proof_of_ink_pot, 1_000_000u32.saturated_into()));

	// Mob Rule pot
	let mob_rule_pot = indiv_pallet_mob_rule::Pallet::<T>::mob_rule_pot_id();
	funding_config.push((mob_rule_pot, 1_000_000u32.saturated_into()));

	// Score pot
	let score_pot = indiv_pallet_score::Pallet::<T>::score_pot_id();
	funding_config.push((score_pot, 1_000_000u32.saturated_into()));

	funding_config
}

/// Initial design families for pallet proof of ink.
/// Returns a vector of (family_index, family_kind, family_id) tuples.
fn get_proof_of_ink_design_families() -> Vec<(u16, indiv_pallet_proof_of_ink::FamilyKind, [u8; 32])>
{
	use indiv_pallet_proof_of_ink::FamilyKind;

	vec![
		(0u16, FamilyKind::Designed { count: 10 }, [1u8; 32]),
		(1u16, FamilyKind::Procedural { range: 5 }, [2u8; 32]),
	]
}

/// Initial configuration for pallet proof of ink.
fn get_proof_of_ink_configuration<T: Config>(
) -> indiv_pallet_proof_of_ink::ConfigRecord<BlockNumberFor<T>> {
	indiv_pallet_proof_of_ink::ConfigRecord {
		// Block counts assume 2 seconds block time (30 blocks per minute)
		reroll_timeout: BlockNumberFor::<T>::from(10 * 24 * 60 * 30u32), // 10 days
		fasttrack_count: 8,
		maximum: 2000,
		full_alloc_len: 64 * 1024 * 1024,
		full_alloc_count: 32,
		init_alloc_len: 2 * 1024 * 1024,
		init_alloc_count: 16,
		timeout: BlockNumberFor::<T>::from(7 * 24 * 60 * 30u32), // 7 days
	}
}

/// Game schedules for initial games.
/// First game on 01.12.2025 at 12:00 UTC, then 6 weekly games, followed by bi-weekly games for a
/// year.
fn get_game_schedules<T: indiv_pallet_game::Config>() -> Vec<GameScheduleOf<T>> {
	let mut schedules = Vec::new();

	let timestamps = {
		// October 6, 2025 12:00 UTC (first Monday) - timestamp: 1759744800
		let first_game_timestamp = 1759744800u32;

		// First 4 weekly games
		let mut t = (0..4)
			.map(|i| first_game_timestamp + (i * 7 * 24 * 60 * 60))
			.collect::<Vec<u32>>();

		// Monthly games on first Monday of each month for 11 months
		t.extend_from_slice(&[
			1762167600u32, // November 3, 2025 12:00 UTC
			1765191600u32, // December 8, 2025 12:00 UTC
			1767610800u32, // January 5, 2026 12:00 UTC
			1770030000u32, // February 2, 2026 12:00 UTC
			1772449200u32, // March 2, 2026 12:00 UTC
			1775469600u32, // April 6, 2026 12:00 UTC
			1777888800u32, // May 4, 2026 12:00 UTC
			1780912800u32, // June 8, 2026 12:00 UTC
			1783332000u32, // July 6, 2026 12:00 UTC
			1785751200u32, // August 3, 2026 12:00 UTC
			1788775200u32, // September 7, 2026 12:00 UTC
		]);

		t
	};

	for timestamp in timestamps {
		schedules.push(GameScheduleOf::<T> {
			game_play_time: timestamp,
			rounds: 3,
			max_group_size: 6,
			..Default::default()
		});
	}

	schedules
}

/// Initial onboarding size for pallet people.
fn get_initial_onboarding_size() -> u32 {
	10
}

/// Initial payout rounds configuration for pallet mob rule.
fn get_initial_mob_rule_payout_rounds_config<T: Config>() -> (u32, u32, BlockNumberFor<T>) {
	let amount_per_round = 1000u32;
	let count = 10u32;
	let period = BlockNumberFor::<T>::from(7 * 24 * 60 * 30u32); // 1 week in blocks (assuming 2 seconds block time)

	(amount_per_round, count, period)
}

/// Initial payout rounds configuration for pallet score.
fn get_initial_score_payout_rounds_config<T: Config>() -> (u32, u32, BlockNumberFor<T>) {
	let amount_per_round = 500u32;
	let count = 5u32;
	let duration = BlockNumberFor::<T>::from(14 * 24 * 60 * 30u32); // 2 weeks in blocks (assuming 2 seconds block time)

	(amount_per_round, count, duration)
}

/// Configuration for the transfer asset.
/// Returns (asset_id, owner_account, is_sufficient, min_balance).
fn get_transfer_asset_configuration<T: Config>() -> (
	<T::Assets as FungiblesInspect<T::AccountId>>::AssetId,
	T::AccountId,
	bool,
	<T::Assets as FungiblesInspect<T::AccountId>>::Balance,
) {
	let multilocation_asset_id: <T::Assets as FungiblesInspect<T::AccountId>>::AssetId =
		T::TransferAssetForeignId::get().into();

	let owner = T::AssetsDestAccount::get();
	let is_sufficient = true;
	let min_balance = 10000u32.saturated_into();

	(multilocation_asset_id, owner, is_sufficient, min_balance)
}

fn reserve_location_for_transfer_asset<T: Config>() -> Result<Location, DispatchError> {
	let foreign_location = T::TransferAssetForeignId::get();
	let (parents, interior) = foreign_location.unpack();
	if parents != 1 {
		return Err(DispatchError::Other("Foreign location must have exactly 1 parent"));
	}
	match interior.first() {
		Some(Junction::Parachain(para_id)) => Ok(Location::new(1, [Parachain(*para_id)])),
		Some(_) => Err(DispatchError::Other("First junction must be Parachain")),
		None => Err(DispatchError::Other("Foreign location has no junctions")),
	}
}

fn set_transfer_asset_reserves<T: Config>(
	asset_id: &<T::Assets as FungiblesInspect<T::AccountId>>::AssetId,
	owner: &T::AccountId,
) -> Result<(), DispatchError> {
	let reserve_location = reserve_location_for_transfer_asset::<T>()?;
	let reserve_data = T::ReserveData::from((reserve_location, false));
	let reserves =
		BoundedVec::<_, ConstU32<{ pallet_assets::MAX_RESERVES }>>::try_from(vec![reserve_data])
			.map_err(|_| DispatchError::Other("Too many reserves configured"))?;

	T::ReserveSetter::set_reserves(owner, asset_id.clone(), reserves)
}

/// Initial reimbursement values configuration for pallet proof of ink.
/// Returns (referred_values, referrer_values) as BoundedVec of (balance, count) tuples.
fn get_initial_reimbursement_values<T: Config>() -> (
	BoundedVec<
		(indiv_pallet_proof_of_ink::BalanceOf<T>, u32),
		<T as indiv_pallet_proof_of_ink::Config>::MaxReimbursementValues,
	>,
	BoundedVec<
		(indiv_pallet_proof_of_ink::BalanceOf<T>, u32),
		<T as indiv_pallet_proof_of_ink::Config>::MaxReimbursementValues,
	>,
) {
	const DECIMALS: u32 = 1_000_000; // 6 decimals

	// Referred reward values schedule: (value, count).
	// Assuming the asset has 6 decimals.
	// First 50 rewards: 20, next 50: 15, next 100: 10, next 200: 5.
	let referred_values = vec![
		((20 * DECIMALS).saturated_into(), 50u32),
		((15 * DECIMALS).saturated_into(), 50u32),
		((10 * DECIMALS).saturated_into(), 100u32),
		((5 * DECIMALS).saturated_into(), 200u32),
	];

	// Referrer reward values schedule: (value, count).
	// Assuming the asset has 6 decimals.
	// First 50 rewards: 20, next 50: 15, next 100: 10, next 200: 5.
	let referrer_values = vec![
		((20 * DECIMALS).saturated_into(), 50u32),
		((15 * DECIMALS).saturated_into(), 50u32),
		((10 * DECIMALS).saturated_into(), 100u32),
		((5 * DECIMALS).saturated_into(), 200u32),
	];

	let referred_bounded = referred_values
		.try_into()
		.expect("referred values should fit in MaxReimbursementValues");
	let referrer_bounded = referrer_values
		.try_into()
		.expect("referrer values should fit in MaxReimbursementValues");

	(referred_bounded, referrer_bounded)
}

/// Initial attestation allowances for pallet people lite.
/// Returns a vector of (account_id, allowance_count) tuples.
///
/// Note: when changing the length of the returning Vec we need to re-run benchmarks
fn get_people_lite_attestation_allowances<T: Config>() -> Vec<(T::AccountId, u32)>
where
	T::AccountId: From<sp_runtime::AccountId32>,
{
	use hex_literal::hex;
	use sp_runtime::AccountId32;

	vec![
		(
			AccountId32::from(hex!(
				"0000000000000000000000000000000000000000000000000000000000000001"
			))
			.into(),
			100u32,
		),
		(
			AccountId32::from(hex!(
				"0000000000000000000000000000000000000000000000000000000000000002"
			))
			.into(),
			50u32,
		),
		(
			AccountId32::from(hex!(
				"0000000000000000000000000000000000000000000000000000000000000003"
			))
			.into(),
			25u32,
		),
	]
}
