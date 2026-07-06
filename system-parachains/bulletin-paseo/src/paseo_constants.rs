// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-only

//! Paseo system-parachain runtime constants used by this runtime.
//!
//! Derived from
//! <https://github.com/paseo-network/runtimes/blob/main/system-parachains/constants/src/paseo.rs>,
//! pruned to the items this runtime actually uses, and with the upstream
//! `paseo_runtime_constants` dependency inlined (values from
//! <https://github.com/paseo-network/runtimes/blob/main/relay/paseo/constants/src/lib.rs>).
//!
//! `fee::WeightToFee` is a polynomial (not the upstream `BlockRatioFee`) because this runtime does
//! not implement `pallet_revive::TxConfig`.

/// Inlined subset of the Paseo relay chain runtime constants crate.
mod paseo_runtime_constants {
	pub mod currency {
		use parachains_common::Balance;

		pub const UNITS: Balance = 10_000_000_000;
		pub const DOLLARS: Balance = UNITS;
		pub const CENTS: Balance = DOLLARS / 100;
		pub const MILLICENTS: Balance = CENTS / 1_000;

		/// Paseo relay chain existential deposit (1 PAS).
		pub const EXISTENTIAL_DEPOSIT: Balance = 100 * CENTS;

		pub const fn deposit(items: u32, bytes: u32) -> Balance {
			items as Balance * 20 * DOLLARS + (bytes as Balance) * 100 * MILLICENTS
		}
	}

	pub mod system_parachain {
		pub const ASSET_HUB_ID: u32 = 1000;
		pub const COLLECTIVES_ID: u32 = 1001;
		pub const PEOPLE_ID: u32 = 1004;
	}
}

/// System parachain ids on Paseo.
pub use paseo_runtime_constants::system_parachain;

/// Consensus-related.
pub mod consensus {
	use frame_support::weights::{constants::WEIGHT_REF_TIME_PER_SECOND, Weight};

	/// How many parachain blocks are processed by the relay chain per parent. Limits the
	/// number of blocks authored per slot.
	pub const BLOCK_PROCESSING_VELOCITY: u32 = 1;
	/// Relay chain slot duration, in milliseconds.
	pub const RELAY_CHAIN_SLOT_DURATION_MILLIS: u32 = 6000;

	/// Average expected block time targeted by the parachain. Picked up by `pallet_timestamp` and
	/// `pallet_aura`.
	pub const MILLISECS_PER_BLOCK: u64 = 6000;

	/// 2 seconds of compute with a 6 second average block.
	pub const MAXIMUM_BLOCK_WEIGHT: Weight = Weight::from_parts(
		WEIGHT_REF_TIME_PER_SECOND.saturating_mul(2),
		cumulus_primitives_core::relay_chain::MAX_POV_SIZE as u64,
	);

	/// Parameters enabling async backing functionality.
	pub mod async_backing {
		/// Maximum number of blocks simultaneously accepted by the Runtime, not yet included into
		/// the relay chain.
		pub const UNINCLUDED_SEGMENT_CAPACITY: u32 = 3;
	}
}

/// Time-related.
pub mod time {
	use parachains_common::BlockNumber;

	pub const MINUTES: BlockNumber =
		60_000 / (super::consensus::MILLISECS_PER_BLOCK as BlockNumber);
	pub const HOURS: BlockNumber = MINUTES * 60;
	pub const DAYS: BlockNumber = HOURS * 24;
}

/// Constants relating to PAS.
pub mod currency {
	use parachains_common::Balance;

	/// System parachain existential deposit: 1/10 of the relay chain's.
	pub const EXISTENTIAL_DEPOSIT: Balance =
		super::paseo_runtime_constants::currency::EXISTENTIAL_DEPOSIT / 10;

	/// One "PAS" that a UI would show a user.
	pub const UNITS: Balance = 10_000_000_000;
	pub const CENTS: Balance = UNITS / 100; // 100_000_000
	pub const MILLICENTS: Balance = CENTS / 1_000; // 100_000

	/// Deposit rate for stored data: 1/100 of the relay chain's.
	pub const fn deposit(items: u32, bytes: u32) -> Balance {
		super::paseo_runtime_constants::currency::deposit(items, bytes) / 100
	}
}

/// Constants related to Paseo fee payment.
pub mod fee {
	use frame_support::{
		pallet_prelude::Weight,
		weights::{
			constants::ExtrinsicBaseWeight, FeePolynomial, WeightToFeeCoefficient,
			WeightToFeeCoefficients, WeightToFeePolynomial,
		},
	};
	use parachains_common::Balance;
	use smallvec::smallvec;
	pub use sp_runtime::Perbill;

	/// Cost of every transaction byte at Paseo system parachains: relay `TRANSACTION_BYTE_FEE`
	/// (`10 * MILLICENTS`) divided by 20.
	pub const TRANSACTION_BYTE_FEE: Balance = super::currency::MILLICENTS / 2;

	/// Maps a weight scalar to a fee. Mirrors the relay chain mapping
	/// (`extrinsic_base_weight -> 1/10 CENT`), scaled to 1/100 of that rate for system parachains.
	pub struct WeightToFee;
	impl frame_support::weights::WeightToFee for WeightToFee {
		type Balance = Balance;

		fn weight_to_fee(weight: &Weight) -> Self::Balance {
			let time_poly: FeePolynomial<Balance> = RefTimeToFee::polynomial().into();
			let proof_poly: FeePolynomial<Balance> = ProofSizeToFee::polynomial().into();
			time_poly.eval(weight.ref_time()).max(proof_poly.eval(weight.proof_size()))
		}
	}

	/// Maps the reference time component of `Weight` to a fee.
	pub struct RefTimeToFee;
	impl WeightToFeePolynomial for RefTimeToFee {
		type Balance = Balance;
		fn polynomial() -> WeightToFeeCoefficients<Self::Balance> {
			let p = super::currency::CENTS;
			let q = 100 * Balance::from(ExtrinsicBaseWeight::get().ref_time());
			smallvec![WeightToFeeCoefficient {
				degree: 1,
				negative: false,
				coeff_frac: Perbill::from_rational(p % q, q),
				coeff_integer: p / q,
			}]
		}
	}

	/// Maps the proof size component of `Weight` to a fee.
	pub struct ProofSizeToFee;
	impl WeightToFeePolynomial for ProofSizeToFee {
		type Balance = Balance;
		fn polynomial() -> WeightToFeeCoefficients<Self::Balance> {
			// Map 10kb proof to 1 CENT.
			let p = super::currency::CENTS;
			let q = 10_000;
			smallvec![WeightToFeeCoefficient {
				degree: 1,
				negative: false,
				coeff_frac: Perbill::from_rational(p % q, q),
				coeff_integer: p / q,
			}]
		}
	}
}

pub mod locations {
	use frame_support::parameter_types;
	use xcm::latest::prelude::{Junction::*, Location};

	use super::paseo_runtime_constants;

	parameter_types! {
		pub AssetHubLocation: Location =
			Location::new(1, Parachain(paseo_runtime_constants::system_parachain::ASSET_HUB_ID));
		pub PeopleLocation: Location =
			Location::new(1, Parachain(paseo_runtime_constants::system_parachain::PEOPLE_ID));
	}
}

/// Default XCM version for genesis config.
pub mod xcm_version {
	pub const SAFE_XCM_VERSION: u32 = xcm::prelude::XCM_VERSION;
}
