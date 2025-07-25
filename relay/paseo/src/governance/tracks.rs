// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Paseo.

// Paseo is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Paseo is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Paseo. If not, see <http://www.gnu.org/licenses/>.

//! Track configurations for governance.

use super::*;

use alloc::borrow::Cow;
use sp_runtime::str_array as s;

const fn percent(x: i32) -> sp_arithmetic::FixedI64 {
	sp_arithmetic::FixedI64::from_rational(x as u128, 100)
}
use pallet_referenda::Curve;
const APP_ROOT: Curve = Curve::make_linear(28, 28, percent(100), percent(100));
const SUP_ROOT: Curve = Curve::make_linear(28, 28, percent(100), percent(100));
const APP_STAKING_ADMIN: Curve = Curve::make_linear(17, 28, percent(50), percent(100));
const SUP_STAKING_ADMIN: Curve =
	Curve::make_reciprocal(12, 28, percent(1), percent(0), percent(50));
const APP_TREASURER: Curve = Curve::make_reciprocal(4, 28, percent(80), percent(50), percent(100));
const SUP_TREASURER: Curve = Curve::make_linear(28, 28, percent(0), percent(50));
const APP_FELLOWSHIP_ADMIN: Curve = Curve::make_linear(17, 28, percent(50), percent(100));
const SUP_FELLOWSHIP_ADMIN: Curve =
	Curve::make_reciprocal(12, 28, percent(1), percent(0), percent(50));
const APP_GENERAL_ADMIN: Curve =
	Curve::make_reciprocal(4, 28, percent(80), percent(50), percent(100));
const SUP_GENERAL_ADMIN: Curve =
	Curve::make_reciprocal(7, 28, percent(10), percent(0), percent(50));
const APP_AUCTION_ADMIN: Curve =
	Curve::make_reciprocal(4, 28, percent(80), percent(50), percent(100));
const SUP_AUCTION_ADMIN: Curve =
	Curve::make_reciprocal(7, 28, percent(10), percent(0), percent(50));
const APP_LEASE_ADMIN: Curve = Curve::make_linear(17, 28, percent(50), percent(100));
const SUP_LEASE_ADMIN: Curve = Curve::make_reciprocal(12, 28, percent(1), percent(0), percent(50));
const APP_REFERENDUM_CANCELLER: Curve = Curve::make_linear(17, 28, percent(50), percent(100));
const SUP_REFERENDUM_CANCELLER: Curve =
	Curve::make_reciprocal(12, 28, percent(1), percent(0), percent(50));
const APP_REFERENDUM_KILLER: Curve = Curve::make_linear(17, 28, percent(50), percent(100));
const SUP_REFERENDUM_KILLER: Curve =
	Curve::make_reciprocal(12, 28, percent(1), percent(0), percent(50));
const APP_SMALL_TIPPER: Curve = Curve::make_linear(10, 28, percent(50), percent(100));
const SUP_SMALL_TIPPER: Curve = Curve::make_reciprocal(1, 28, percent(4), percent(0), percent(50));
const APP_BIG_TIPPER: Curve = Curve::make_linear(10, 28, percent(50), percent(100));
const SUP_BIG_TIPPER: Curve = Curve::make_reciprocal(8, 28, percent(1), percent(0), percent(50));
const APP_SMALL_SPENDER: Curve = Curve::make_linear(17, 28, percent(50), percent(100));
const SUP_SMALL_SPENDER: Curve =
	Curve::make_reciprocal(12, 28, percent(1), percent(0), percent(50));
const APP_MEDIUM_SPENDER: Curve = Curve::make_linear(23, 28, percent(50), percent(100));
const SUP_MEDIUM_SPENDER: Curve =
	Curve::make_reciprocal(16, 28, percent(1), percent(0), percent(50));
const APP_BIG_SPENDER: Curve = Curve::make_linear(28, 28, percent(50), percent(100));
const SUP_BIG_SPENDER: Curve = Curve::make_reciprocal(20, 28, percent(1), percent(0), percent(50));
const APP_WHITELISTED_CALLER: Curve =
	Curve::make_reciprocal(16, 28 * 24, percent(96), percent(50), percent(100));
const SUP_WHITELISTED_CALLER: Curve =
	Curve::make_reciprocal(1, 28, percent(20), percent(5), percent(50));

const TRACKS_DATA: [pallet_referenda::Track<u16, Balance, BlockNumber>; 16] = [
	pallet_referenda::Track {
		id: 0,
		info: pallet_referenda::TrackInfo {
			name: s("root"),
			max_deciding: 1,
			decision_deposit: 1000 * GRAND,
			prepare_period: 2 * HOURS,
			decision_period: 28 * DAYS,
			confirm_period: 24 * HOURS,
			min_enactment_period: 24 * HOURS,
			min_approval: APP_ROOT,
			min_support: SUP_ROOT,
		},
	},
	pallet_referenda::Track {
		id: 1,
		info: pallet_referenda::TrackInfo {
			name: s("whitelisted_caller"),
			max_deciding: 100,
			decision_deposit: 10 * GRAND,
			prepare_period: 30 * MINUTES,
			decision_period: 28 * DAYS,
			confirm_period: 10 * MINUTES,
			min_enactment_period: 10 * MINUTES,
			min_approval: APP_WHITELISTED_CALLER,
			min_support: SUP_WHITELISTED_CALLER,
		},
	},
	pallet_referenda::Track {
		id: 2,
		info: pallet_referenda::TrackInfo {
			name: s("wish_for_change"),
			max_deciding: 10,
			decision_deposit: 20 * GRAND,
			prepare_period: 2 * HOURS,
			decision_period: 28 * DAYS,
			confirm_period: 24 * HOURS,
			min_enactment_period: 10 * MINUTES,
			min_approval: APP_ROOT,
			min_support: SUP_ROOT,
		},
	},
	pallet_referenda::Track {
		id: 10,
		info: pallet_referenda::TrackInfo {
			name: s("staking_admin"),
			max_deciding: 10,
			decision_deposit: 5 * GRAND,
			prepare_period: 2 * HOURS,
			decision_period: 28 * DAYS,
			confirm_period: 3 * HOURS,
			min_enactment_period: 10 * MINUTES,
			min_approval: APP_STAKING_ADMIN,
			min_support: SUP_STAKING_ADMIN,
		},
	},
	pallet_referenda::Track {
		id: 11,
		info: pallet_referenda::TrackInfo {
			name: s("treasurer"),
			max_deciding: 10,
			decision_deposit: GRAND,
			prepare_period: 2 * HOURS,
			decision_period: 28 * DAYS,
			confirm_period: 7 * DAYS,
			min_enactment_period: 24 * HOURS,
			min_approval: APP_TREASURER,
			min_support: SUP_TREASURER,
		},
	},
	pallet_referenda::Track {
		id: 12,
		info: pallet_referenda::TrackInfo {
			name: s("lease_admin"),
			max_deciding: 10,
			decision_deposit: 5 * GRAND,
			prepare_period: 2 * HOURS,
			decision_period: 28 * DAYS,
			confirm_period: 3 * HOURS,
			min_enactment_period: 10 * MINUTES,
			min_approval: APP_LEASE_ADMIN,
			min_support: SUP_LEASE_ADMIN,
		},
	},
	pallet_referenda::Track {
		id: 13,
		info: pallet_referenda::TrackInfo {
			name: s("fellowship_admin"),
			max_deciding: 10,
			decision_deposit: 5 * GRAND,
			prepare_period: 2 * HOURS,
			decision_period: 28 * DAYS,
			confirm_period: 3 * HOURS,
			min_enactment_period: 10 * MINUTES,
			min_approval: APP_FELLOWSHIP_ADMIN,
			min_support: SUP_FELLOWSHIP_ADMIN,
		},
	},
	pallet_referenda::Track {
		id: 14,
		info: pallet_referenda::TrackInfo {
			name: s("general_admin"),
			max_deciding: 10,
			decision_deposit: 5 * GRAND,
			prepare_period: 2 * HOURS,
			decision_period: 28 * DAYS,
			confirm_period: 3 * HOURS,
			min_enactment_period: 10 * MINUTES,
			min_approval: APP_GENERAL_ADMIN,
			min_support: SUP_GENERAL_ADMIN,
		},
	},
	pallet_referenda::Track {
		id: 15,
		info: pallet_referenda::TrackInfo {
			name: s("auction_admin"),
			max_deciding: 10,
			decision_deposit: 5 * GRAND,
			prepare_period: 2 * HOURS,
			decision_period: 28 * DAYS,
			confirm_period: 3 * HOURS,
			min_enactment_period: 10 * MINUTES,
			min_approval: APP_AUCTION_ADMIN,
			min_support: SUP_AUCTION_ADMIN,
		},
	},
	pallet_referenda::Track {
		id: 20,
		info: pallet_referenda::TrackInfo {
			name: s("referendum_canceller"),
			max_deciding: 1_000,
			decision_deposit: 10 * GRAND,
			prepare_period: 2 * HOURS,
			decision_period: 7 * DAYS,
			confirm_period: 3 * HOURS,
			min_enactment_period: 10 * MINUTES,
			min_approval: APP_REFERENDUM_CANCELLER,
			min_support: SUP_REFERENDUM_CANCELLER,
		},
	},
	pallet_referenda::Track {
		id: 21,
		info: pallet_referenda::TrackInfo {
			name: s("referendum_killer"),
			max_deciding: 1_000,
			decision_deposit: 50 * GRAND,
			prepare_period: 2 * HOURS,
			decision_period: 28 * DAYS,
			confirm_period: 3 * HOURS,
			min_enactment_period: 10 * MINUTES,
			min_approval: APP_REFERENDUM_KILLER,
			min_support: SUP_REFERENDUM_KILLER,
		},
	},
	pallet_referenda::Track {
		id: 30,
		info: pallet_referenda::TrackInfo {
			name: s("small_tipper"),
			max_deciding: 200,
			decision_deposit: DOLLARS,
			prepare_period: MINUTES,
			decision_period: 7 * DAYS,
			confirm_period: 10 * MINUTES,
			min_enactment_period: MINUTES,
			min_approval: APP_SMALL_TIPPER,
			min_support: SUP_SMALL_TIPPER,
		},
	},
	pallet_referenda::Track {
		id: 31,
		info: pallet_referenda::TrackInfo {
			name: s("big_tipper"),
			max_deciding: 100,
			decision_deposit: 10 * DOLLARS,
			prepare_period: 10 * MINUTES,
			decision_period: 7 * DAYS,
			confirm_period: HOURS,
			min_enactment_period: 10 * MINUTES,
			min_approval: APP_BIG_TIPPER,
			min_support: SUP_BIG_TIPPER,
		},
	},
	pallet_referenda::Track {
		id: 32,
		info: pallet_referenda::TrackInfo {
			name: s("small_spender"),
			max_deciding: 50,
			decision_deposit: 100 * DOLLARS,
			prepare_period: 4 * HOURS,
			decision_period: 28 * DAYS,
			confirm_period: 2 * DAYS,
			min_enactment_period: 24 * HOURS,
			min_approval: APP_SMALL_SPENDER,
			min_support: SUP_SMALL_SPENDER,
		},
	},
	pallet_referenda::Track {
		id: 33,
		info: pallet_referenda::TrackInfo {
			name: s("medium_spender"),
			max_deciding: 50,
			decision_deposit: 200 * DOLLARS,
			prepare_period: 4 * HOURS,
			decision_period: 28 * DAYS,
			confirm_period: 4 * DAYS,
			min_enactment_period: 24 * HOURS,
			min_approval: APP_MEDIUM_SPENDER,
			min_support: SUP_MEDIUM_SPENDER,
		},
	},
	pallet_referenda::Track {
		id: 34,
		info: pallet_referenda::TrackInfo {
			name: s("big_spender"),
			max_deciding: 50,
			decision_deposit: 400 * DOLLARS,
			prepare_period: 4 * HOURS,
			decision_period: 28 * DAYS,
			confirm_period: 7 * DAYS,
			min_enactment_period: 24 * HOURS,
			min_approval: APP_BIG_SPENDER,
			min_support: SUP_BIG_SPENDER,
		},
	},
];

pub struct TracksInfo;
impl pallet_referenda::TracksInfo<Balance, BlockNumber> for TracksInfo {
	type Id = u16;
	type RuntimeOrigin = <RuntimeOrigin as frame_support::traits::OriginTrait>::PalletsOrigin;
	fn tracks(
	) -> impl Iterator<Item = Cow<'static, pallet_referenda::Track<Self::Id, Balance, BlockNumber>>>
	{
		TRACKS_DATA.iter().map(Cow::Borrowed)
	}
	fn track_for(id: &Self::RuntimeOrigin) -> Result<Self::Id, ()> {
		if let Ok(system_origin) = frame_system::RawOrigin::try_from(id.clone()) {
			match system_origin {
				frame_system::RawOrigin::Root => Ok(0),
				_ => Err(()),
			}
		} else if let Ok(custom_origin) = origins::Origin::try_from(id.clone()) {
			match custom_origin {
				origins::Origin::WhitelistedCaller => Ok(1),
				origins::Origin::WishForChange => Ok(2),
				// General admin
				origins::Origin::StakingAdmin => Ok(10),
				origins::Origin::Treasurer => Ok(11),
				origins::Origin::LeaseAdmin => Ok(12),
				origins::Origin::FellowshipAdmin => Ok(13),
				origins::Origin::GeneralAdmin => Ok(14),
				origins::Origin::AuctionAdmin => Ok(15),
				// Referendum admins
				origins::Origin::ReferendumCanceller => Ok(20),
				origins::Origin::ReferendumKiller => Ok(21),
				// Limited treasury spenders
				origins::Origin::SmallTipper => Ok(30),
				origins::Origin::BigTipper => Ok(31),
				origins::Origin::SmallSpender => Ok(32),
				origins::Origin::MediumSpender => Ok(33),
				origins::Origin::BigSpender => Ok(34),
			}
		} else {
			Err(())
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	parameter_types! {
		const Linear100Percent: Curve = Curve::make_linear(28, 28, percent(100), percent(100));
	}

	#[test]
	fn ensure_root_track_configuration() {
		let root_track = TRACKS_DATA.get(0).unwrap();
		assert_eq!(root_track.id, 0);
		// Ensure Root track decision deposit is 1M.
		assert_eq!(root_track.info.decision_deposit, 1000*GRAND);
		// Ensure Root track is configured with a linear curve set at 100%
		// for both support and approval.
		assert_eq!(
			root_track.info.min_approval, Linear100Percent::get()
		);
		assert_eq!(
			root_track.info.min_support, Linear100Percent::get()
		);
	}

	#[test]
	fn ensure_approval_curve_root() {
		assert_eq!(APP_ROOT, Linear100Percent::get());
	}

	#[test]
	fn ensure_support_curve_root() {
		assert_eq!(APP_ROOT, Linear100Percent::get());
	}
}