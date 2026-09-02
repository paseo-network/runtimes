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

//! The pot and load-deposit accounting of sponsored instances.
//!
//! A sponsored instance's load-side costs are underwritten by its pot: a keyless account funded
//! permissionlessly through [`Pallet::fund_pot`]. Every sponsored load takes a per-key deposit
//! hold on the pot at the current [`Config::LoadDeposit`], recorded on the instance's ledger
//! ([`InstanceRecord::current_load_deposit`] and [`InstanceRecord::old_load_deposit`]) and
//! released back to the pot's free balance once the key stops occupying the chain: at unload, or
//! at the cleanup of its expired recycler ([`Pallet::clean_recycler`]) for a key that was never
//! unloaded.

use crate::*;
use alloc::vec::Vec;
use frame_support::{
	pallet_prelude::*,
	traits::{
		fungibles::{Inspect as _, InspectHold as _, Mutate as _, MutateHold as _},
		tokens::{Fortitude, Precision, Preservation},
		AccountTouch as _, Defensive as _,
	},
};
use sp_crypto_hashing::blake2_256;
use sp_runtime::{
	traits::{CheckedAdd, CheckedDiv, CheckedMul, CheckedSub, TrailingZeroInput, Zero},
	ArithmeticError, SaturatedConversion,
};

/// Prefix for deriving a sponsored instance's pot account from its instance id.
pub const POT_PREFIX: [u8; 16] = *b"coinage/loadpot!";

/// The view of a sponsored instance's pot. Returned by the pallet view
/// [`Pallet::get_pot_status`].
#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, TypeInfo)]
pub struct PotView<AssetId, Balance> {
	/// The chain-wide deposit currency and price ([`Config::LoadDeposit`]).
	pub load_deposit: (AssetId, Balance),
	/// The pot's free balance in the current deposit asset.
	pub free_in_deposit_asset: Balance,
	/// The number of loads `free_in_deposit_asset` can back at the current price: what tells a
	/// wallet whether to offer loading, and a sponsor when to top up.
	pub loads_affordable: u32,
	/// The number of keys whose deposits are live: loaded, minus unloaded, minus cleaned up with
	/// their expired recycler.
	pub redeemable_keys: u32,
	/// The tier the instance last loaded at.
	pub current_tier: Option<DepositTier<AssetId, Balance>>,
	/// The superseded tier still holding deposits. While it is `Some`, a deposit change
	/// blocks loading with [`CustomInvalidity::LoadDepositOldTierOccupied`].
	pub old_tier: Option<DepositTier<AssetId, Balance>>,
	/// `None`, or the invalidity the next load would come back with.
	pub loads_blocked: Option<CustomInvalidity>,
}

impl<T: Config> Pallet<T> {
	/// The account underwriting the load deposits of the sponsored instance `instance_id`.
	// We use our own derivation because AccountIdConversion would require to change the mock.rs
	pub fn pot_account(instance_id: InstanceId) -> T::AccountId {
		let entropy = (POT_PREFIX, instance_id).using_encoded(blake2_256);
		Decode::decode(&mut TrailingZeroInput::new(entropy.as_ref()))
			.expect("infinite length input; no invalid inputs for type; qed")
	}

	/// The status of a sponsored instance's pot, `None` for a sufficient or missing instance.
	pub(crate) fn pot_status(
		instance_id: InstanceId,
	) -> Option<PotView<FungiblesAssetIdOf<T>, FungiblesBalanceOf<T>>> {
		let record = Instances::<T>::get(instance_id)?;
		if record.mode != InstanceMode::Sponsored {
			return None;
		}

		let pot = Self::pot_account(instance_id);
		let load_deposit = T::LoadDeposit::get();
		let redeemable_keys = record.load_deposit_key_count();

		let (deposit_asset_id, price) = &load_deposit;
		let free_in_deposit_asset = Self::pot_withdrawable(&pot, deposit_asset_id);
		// A zero price is rejected by the integrity test, so this only guards a chain that
		// has none.
		let loads_affordable = free_in_deposit_asset
			.checked_div(price)
			.map(SaturatedConversion::saturated_into::<u32>)
			.unwrap_or(0);

		// This is the check validation runs, so the view reports exactly what a load would hit.
		let loads_blocked = Self::ensure_can_charge_load_deposit(instance_id, 1).err();

		Some(PotView {
			load_deposit,
			free_in_deposit_asset,
			loads_affordable,
			redeemable_keys,
			current_tier: record.current_load_deposit,
			old_tier: record.old_load_deposit,
			loads_blocked,
		})
	}

	/// Per currency: the funder's recorded pot contribution and how much of it is
	/// withdrawable right now, which is the contribution capped by what
	/// [`Pallet::withdraw_pot_funds`] could actually move out of the pot in that currency.
	///
	/// The pot's account for a funded currency can never be dusted: withdrawals move funds
	/// with [`Preservation::Preserve`], and a load-deposit hold also leaves the minimum
	/// balance untouchable in free balance. The flip side is that the last minimum balance's
	/// worth in a currency is never withdrawable, whoever contributed it.
	pub(crate) fn pot_contributions(
		instance_id: InstanceId,
		funder: T::AccountId,
	) -> Vec<(FungiblesAssetIdOf<T>, FungiblesBalanceOf<T>, FungiblesBalanceOf<T>)> {
		let pot = Self::pot_account(instance_id);
		PotContributions::<T>::iter_prefix((instance_id, funder))
			.map(|(deposit_asset_id, contribution)| {
				let free = Self::pot_withdrawable(&pot, &deposit_asset_id);
				(deposit_asset_id, contribution, contribution.min(free))
			})
			.collect()
	}

	/// Transfer `amount` of `deposit_asset_id` from `funder` to the pot of `instance_id` and record
	/// the contribution.
	///
	/// Touches the pot's account for the currency if needed, at the funder's expense; the touch
	/// cost is not part of the withdrawable contribution. `amount` must be at least the
	/// currency's minimum balance, so the transfer cannot be dusted on arrival.
	///
	/// Storage must be reverted on error.
	pub(crate) fn do_fund_pot(
		funder: &T::AccountId,
		instance_id: InstanceId,
		deposit_asset_id: FungiblesAssetIdOf<T>,
		amount: FungiblesBalanceOf<T>,
	) -> DispatchResult {
		ensure!(!amount.is_zero(), Error::<T>::ZeroAmount);
		let record = Self::instance(instance_id)?;
		ensure!(record.mode == InstanceMode::Sponsored, Error::<T>::InstanceNotSponsored);

		let pot = Self::pot_account(instance_id);

		// Touch the pot for the asset id.
		if T::Fungibles::should_touch(deposit_asset_id.clone(), &pot) {
			T::Fungibles::touch(deposit_asset_id.clone(), &pot, funder)?;
		}

		// A funding below the currency's minimum balance could be dusted by the transfer right
		// away, so refuse it outright. Everything at or above it is safe for good: withdrawals
		// move funds with `Preserve` and holds leave the minimum balance untouchable in free
		// balance, so neither can dust the pot later.
		ensure!(
			amount >= T::Fungibles::minimum_balance(deposit_asset_id.clone()),
			Error::<T>::FundingBelowMinimumBalance
		);

		T::Fungibles::transfer(
			deposit_asset_id.clone(),
			funder,
			&pot,
			amount,
			Preservation::Protect,
		)?;

		PotContributions::<T>::try_mutate(
			(instance_id, funder, &deposit_asset_id),
			|contribution| -> DispatchResult {
				*contribution =
					contribution.checked_add(&amount).ok_or(ArithmeticError::Overflow)?;
				Ok(())
			},
		)?;

		Self::deposit_event(Event::PotFunded {
			instance_id,
			funder: funder.clone(),
			currency: deposit_asset_id,
			amount,
		});
		Ok(())
	}

	/// Transfer up to `funder`'s recorded contribution in `deposit_asset_id` back out of the
	/// pot of `instance_id`.
	///
	/// Storage must be reverted on error.
	pub(crate) fn do_withdraw_pot_funds(
		funder: &T::AccountId,
		instance_id: InstanceId,
		deposit_asset_id: FungiblesAssetIdOf<T>,
		amount: FungiblesBalanceOf<T>,
	) -> DispatchResult {
		ensure!(!amount.is_zero(), Error::<T>::ZeroAmount);

		let contribution = PotContributions::<T>::get((instance_id, funder, &deposit_asset_id));
		let remaining = contribution
			.checked_sub(&amount)
			.ok_or(Error::<T>::WithdrawExceedsContribution)?;

		let pot = Self::pot_account(instance_id);
		T::Fungibles::transfer(
			deposit_asset_id.clone(),
			&pot,
			funder,
			amount,
			Preservation::Preserve,
		)?;

		if remaining.is_zero() {
			PotContributions::<T>::remove((instance_id, funder, &deposit_asset_id));
		} else {
			PotContributions::<T>::insert((instance_id, funder, &deposit_asset_id), remaining);
		}

		Self::deposit_event(Event::PotFundsWithdrawn {
			instance_id,
			funder: funder.clone(),
			currency: deposit_asset_id,
			amount,
		});

		Ok(())
	}

	/// Re-price every live load deposit of `instance_id` to the current
	/// [`Config::LoadDeposit`].
	///
	/// Storage must be reverted on error.
	pub(crate) fn do_collapse_load_deposits(instance_id: InstanceId) -> DispatchResult {
		let (deposit_asset_id, price) = T::LoadDeposit::get();
		let pot = Self::pot_account(instance_id);

		let mut record = Self::instance(instance_id)?;
		ensure!(record.mode == InstanceMode::Sponsored, Error::<T>::InstanceNotSponsored);

		let needs_collapse = record.old_load_deposit.is_some() ||
			record
				.current_load_deposit
				.as_ref()
				.is_some_and(|c| !c.is_priced_at(&deposit_asset_id, &price));
		ensure!(needs_collapse, Error::<T>::NothingToCollapse);

		// Release every hold and take the whole re-priced requirement fresh.
		for tier in record.old_load_deposit.iter().chain(record.current_load_deposit.iter()) {
			let total =
				tier.price.checked_mul(&tier.count.into()).ok_or(ArithmeticError::Overflow)?;
			let _ = T::Fungibles::release(
				tier.asset_id.clone(),
				&HoldReason::LoadDeposit.into(),
				&pot,
				total,
				Precision::Exact,
			)
			.defensive_proof("coinage: the load deposit hold covers every ledger unit");
		}

		let count = record.load_deposit_key_count();
		let required = price.checked_mul(&count.into()).ok_or(ArithmeticError::Overflow)?;
		T::Fungibles::hold(
			deposit_asset_id.clone(),
			&HoldReason::LoadDeposit.into(),
			&pot,
			required,
		)
		.map_err(|_| Error::<T>::PotCannotCoverLoadDeposit)?;

		record.current_load_deposit =
			Some(DepositTier { asset_id: deposit_asset_id.clone(), price, count });
		record.old_load_deposit = None;
		Instances::<T>::insert(instance_id, record);

		Self::deposit_event(Event::LoadDepositsCollapsed {
			instance_id,
			currency: deposit_asset_id,
			price,
			count,
		});

		Ok(())
	}

	/// How much of `deposit_asset_id` [`Pallet::withdraw_pot_funds`] could move out of `pot` right
	/// now, holds excluded.
	fn pot_withdrawable(
		pot: &T::AccountId,
		deposit_asset_id: &FungiblesAssetIdOf<T>,
	) -> FungiblesBalanceOf<T> {
		T::Fungibles::reducible_balance(
			deposit_asset_id.clone(),
			pot,
			Preservation::Preserve,
			Fortitude::Polite,
		)
	}

	/// Validity-side check that a sponsored load of `n` keys can be collateralized: the
	/// ledger has room for a rotation and the pot can cover the hold.
	///
	/// No-op for a sufficient instance. The solvency check is against the pot's free
	/// balance without crediting any release the same call may perform: validity must not
	/// depend on the tiers a settlement would walk.
	pub(crate) fn ensure_can_charge_load_deposit(
		instance_id: InstanceId,
		n: u32,
	) -> Result<(), CustomInvalidity> {
		let record = Instances::<T>::get(instance_id).ok_or(CustomInvalidity::InstanceNotFound)?;
		if record.mode == InstanceMode::Sufficient || n == 0 {
			return Ok(());
		}

		let (deposit_asset_id, price) = T::LoadDeposit::get();
		if let Some(current) = &record.current_load_deposit {
			let rotates = !current.is_priced_at(&deposit_asset_id, &price);
			// Refusing a rotation on a taken slot is deliberate: merging tiers would invent
			// prices governance never set (and needs a rate across currencies), and
			// collapsing automatically would spike one load's funding requirement to the
			// whole re-pricing delta.
			if rotates && record.old_load_deposit.is_some() {
				return Err(CustomInvalidity::LoadDepositOldTierOccupied);
			}
		}
		let amount = price
			.checked_mul(&n.into())
			.ok_or(CustomInvalidity::PotCannotCoverLoadDeposit)?;
		let pot = Self::pot_account(instance_id);
		// `can_hold` also covers the pot having no account for `deposit_asset_id` at all, so a
		// currency switch degrades to the same invalidity rather than a new failure mode.
		if !T::Fungibles::can_hold(deposit_asset_id, &HoldReason::LoadDeposit.into(), &pot, amount)
		{
			return Err(CustomInvalidity::PotCannotCoverLoadDeposit);
		}
		Ok(())
	}

	/// The weight [`Pallet::charge_load_deposit`] consumes on `instance_id`: the benchmarked
	/// charge for a sponsored instance. A sufficient or missing instance charges nothing, but
	/// still pays for the instance read that decides it.
	pub(crate) fn charge_load_deposit_weight(instance_id: InstanceId) -> Weight {
		match Instances::<T>::get(instance_id) {
			Some(record) if record.mode == InstanceMode::Sponsored =>
				T::WeightInfo::charge_load_deposit(),
			_ => T::WeightInfo::read_instance(),
		}
	}

	/// Take the deposits backing `n` freshly loaded keys of `instance_id` from its pot, at
	/// the current [`Config::LoadDeposit`], and record them on the ledger.
	///
	/// No-op for a sufficient instance. Runs once per load call with `n` summed across batch
	/// items, so the charge is per call, not per key.
	///
	/// Storage must be reverted on error.
	pub(crate) fn charge_load_deposit(instance_id: InstanceId, n: u32) -> DispatchResult {
		if n == 0 {
			return Ok(());
		}
		let mut record = Self::instance(instance_id)?;
		if record.mode == InstanceMode::Sufficient {
			return Ok(());
		}

		let (deposit_asset_id, price) = T::LoadDeposit::get();
		match record.current_load_deposit.take() {
			Some(current) if current.is_priced_at(&deposit_asset_id, &price) => {
				let count = current.count.checked_add(n).ok_or(ArithmeticError::Overflow)?;
				record.current_load_deposit = Some(DepositTier { count, ..current });
			},
			current => {
				// The load rotates the current tier into the old slot, which must be free.
				// Validation performs the same check, but dispatch cannot rely on it: a load
				// can also be dispatched directly, without the extension's validation.
				if let Some(tier) = current {
					ensure!(
						record.old_load_deposit.is_none(),
						Error::<T>::LoadDepositOldTierOccupied
					);
					record.old_load_deposit = Some(tier);
				}
				record.current_load_deposit =
					Some(DepositTier { asset_id: deposit_asset_id.clone(), price, count: n });
			},
		}
		Instances::<T>::insert(instance_id, record);

		let amount = price.checked_mul(&n.into()).ok_or(ArithmeticError::Overflow)?;
		let pot = Self::pot_account(instance_id);
		T::Fungibles::hold(deposit_asset_id.clone(), &HoldReason::LoadDeposit.into(), &pot, amount)
			.map_err(|_| Error::<T>::PotCannotCoverLoadDeposit)?;

		Self::deposit_event(Event::LoadDepositsHeld {
			instance_id,
			currency: deposit_asset_id,
			price,
			count: n,
		});
		Ok(())
	}

	/// The weight [`Pallet::settle_load_deposits`] consumes on `instance_id`: the benchmarked
	/// settlement for a sponsored instance. A sufficient or missing instance settles nothing, but
	/// still pays for the instance read that decides it.
	pub(crate) fn settle_load_deposit_weight(instance_id: InstanceId) -> Weight {
		match Instances::<T>::get(instance_id) {
			Some(record) if record.mode == InstanceMode::Sponsored =>
				T::WeightInfo::settle_load_deposits(),
			_ => T::WeightInfo::read_instance(),
		}
	}

	/// Release the deposits behind `k` settled keys of `instance_id` to the pot's free balance,
	/// oldest tier first.
	///
	/// No-op for a sufficient instance. Never fails: the exits this backs must always go
	/// through, so an inconsistent ledger is logged and settled as far as it goes.
	pub(crate) fn settle_load_deposits(instance_id: InstanceId, k: u32) {
		let Some(mut record) = Instances::<T>::get(instance_id) else {
			log::error!(
				target: LOG_TARGET,
				"settling load deposits of missing instance {instance_id}"
			);
			return;
		};
		if record.mode == InstanceMode::Sufficient || k == 0 {
			return;
		}
		let mut remaining = k;

		if let Some(old) = &mut record.old_load_deposit {
			let taken = old.count.min(remaining);
			Self::release_load_deposit(instance_id, old, taken);
			old.count -= taken;
			remaining -= taken;
			// Dropping the drained tier is what frees the slot the next rotation needs.
			if old.count == 0 {
				record.old_load_deposit = None;
			}
		}

		if remaining > 0 {
			if let Some(current) = &mut record.current_load_deposit {
				let taken = current.count.min(remaining);
				Self::release_load_deposit(instance_id, current, taken);
				current.count -= taken;
				remaining -= taken;
				// Like the old tier, a drained current tier is dropped.
				if current.count == 0 {
					record.current_load_deposit = None;
				}
			}
		}

		Instances::<T>::insert(instance_id, record);

		if remaining > 0 {
			// Not an inconsistency: keys loaded while the instance was sufficient carry no
			// deposit (see `make_instance_sponsored`), so their unloads settle nothing once
			// the ledger is drained.
			log::debug!(
				target: LOG_TARGET,
				"load deposit ledger of instance {instance_id} had {remaining} fewer units \
				than the {k} keys settled; uncollateralized keys resolved"
			);
		}
	}

	/// Release `count` units of `tier` from the pot's load-deposit hold to its free balance.
	fn release_load_deposit(
		instance_id: InstanceId,
		tier: &DepositTier<FungiblesAssetIdOf<T>, FungiblesBalanceOf<T>>,
		count: u32,
	) {
		if count == 0 {
			return;
		}
		let Some(amount) = tier.price.checked_mul(&count.into()) else {
			log::error!(
				target: LOG_TARGET,
				"load deposit release amount overflows for instance {instance_id}; \
				hold left in place"
			);
			return;
		};
		let pot = Self::pot_account(instance_id);
		if let Err(e) = T::Fungibles::release(
			tier.asset_id.clone(),
			&HoldReason::LoadDeposit.into(),
			&pot,
			amount,
			Precision::Exact,
		) {
			// Inconsistent state (the hold backs every ledger unit by construction),
			// not user error; the exit must proceed regardless.
			log::error!(
				target: LOG_TARGET,
				"failed to release load deposit of instance {instance_id}: {e:?}"
			);
			return;
		}

		Self::deposit_event(Event::LoadDepositsReleased {
			instance_id,
			currency: tier.asset_id.clone(),
			amount,
			count,
		});
	}
}
