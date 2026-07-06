// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-only

use super::{
	AccountId, AllPalletsWithSystem, Balance, Balances, BaseDeliveryFee, FeeAssetId, ParachainInfo,
	ParachainSystem, PolkadotXcm, Runtime, RuntimeCall, RuntimeEvent, RuntimeHoldReason,
	RuntimeOrigin, TransactionByteFee, WeightToFee, XcmpQueue,
};
use crate::paseo_constants::system_parachain::{ASSET_HUB_ID, COLLECTIVES_ID};
use alloc::{vec, vec::Vec};
use frame_support::{
	parameter_types,
	traits::{
		fungible::HoldConsideration, tokens::imbalance::ResolveTo, ConstU32, Contains, Equals,
		Everything, EverythingBut, LinearStoragePrice, Nothing,
	},
};
use frame_system::EnsureRoot;
use pallet_collator_selection::StakingPotAccountId;
use pallet_xcm::{AuthorizedAliasers, XcmPassthrough};
use parachains_common::{
	xcm_config::{
		AliasAccountId32FromSiblingSystemChain, AllSiblingSystemParachains,
		ConcreteAssetFromSystem, ParentRelayOrSiblingParachains, RelayOrOtherSystemParachains,
	},
	TREASURY_PALLET_ID,
};
use polkadot_parachain_primitives::primitives::Sibling;
use polkadot_runtime_common::xcm_sender::ExponentialPrice;
use sp_runtime::traits::AccountIdConversion;
use xcm::latest::prelude::*;
use xcm_builder::{
	AccountId32Aliases, AliasChildLocation, AliasOriginRootUsingFilter,
	AllowExplicitUnpaidExecutionFrom, AllowHrmpNotificationsFromRelayChain,
	AllowKnownQueryResponses, AllowSubscriptionsFrom, AllowTopLevelPaidExecutionFrom,
	DescribeAllTerminal, DescribeFamily, EnsureXcmOrigin, FrameTransactionalProcessor,
	FungibleAdapter, HashedDescription, IsConcrete, LocationAsSuperuser, ParentIsPreset,
	RelayChainAsNative, SendXcmFeeToAccount, SiblingParachainAsNative, SiblingParachainConvertsVia,
	SignedAccountId32AsNative, SignedToAccountId32, SovereignSignedViaLocation, TakeWeightCredit,
	TrailingSetTopicAsId, UsingComponents, WeightInfoBounds, WithComputedOrigin, WithUniqueTopic,
	XcmFeeManagerFromComponents,
};
use xcm_executor::XcmExecutor;

// Re-export
pub use crate::paseo_constants::locations::PeopleLocation;

/// The genesis hash of the Paseo testnet relay chain. Used to identify it over XCM.
///
/// Not yet exposed by the Polkadot SDK, so we define it locally. Matches
/// `0x77afd6190f1554ad45fd0d31aee62aacc33c6db0ea801129acb813f913e0764f`.
pub const PASEO_GENESIS_HASH: [u8; 32] = [
	0x77, 0xaf, 0xd6, 0x19, 0x0f, 0x15, 0x54, 0xad, 0x45, 0xfd, 0x0d, 0x31, 0xae, 0xe6, 0x2a, 0xac,
	0xc3, 0x3c, 0x6d, 0xb0, 0xea, 0x80, 0x11, 0x29, 0xac, 0xb8, 0x13, 0xf9, 0x13, 0xe0, 0x76, 0x4f,
];

parameter_types! {
	pub const RootLocation: Location = Location::here();
	pub const TokenRelayLocation: Location = Location::parent();
	pub AssetHubLocation: Location = Location::new(1, [Parachain(ASSET_HUB_ID)]);
	pub const RelayNetwork: Option<NetworkId> = Some(NetworkId::ByGenesis(PASEO_GENESIS_HASH));
	pub RelayChainOrigin: RuntimeOrigin = cumulus_pallet_xcm::Origin::Relay.into();
	pub UniversalLocation: InteriorLocation =
		[GlobalConsensus(RelayNetwork::get().unwrap()), Parachain(ParachainInfo::parachain_id().into())].into();
	pub const MaxInstructions: u32 = 100;
	pub const MaxAssetsIntoHolding: u32 = 64;
	pub FellowshipLocation: Location = Location::new(1, Parachain(COLLECTIVES_ID));
}

/// Type for specifying how a `Location` can be converted into an `AccountId`. This is used
/// when determining ownership of accounts for asset transacting and when attempting to use XCM
/// `Transact` in order to determine the dispatch Origin.
pub type LocationToAccountId = (
	// The parent (Relay-chain) origin converts to the parent `AccountId`.
	ParentIsPreset<AccountId>,
	// Sibling parachain origins convert to AccountId via the `ParaId::into`.
	SiblingParachainConvertsVia<Sibling, AccountId>,
	// Straight up local `AccountId32` origins just alias directly to `AccountId`.
	AccountId32Aliases<RelayNetwork, AccountId>,
	// Foreign locations alias into accounts according to a hash of their standard description.
	HashedDescription<AccountId, DescribeFamily<DescribeAllTerminal>>,
);

/// Means for transacting the native currency on this chain.
pub type FungibleTransactor = FungibleAdapter<
	// Use this currency:
	Balances,
	// Use this currency when it is a fungible asset matching the given location or name:
	IsConcrete<TokenRelayLocation>,
	// Do a simple punn to convert an `AccountId32` `Location` into a native chain
	// `AccountId`:
	LocationToAccountId,
	// Our chain's `AccountId` type (we can't get away without mentioning it explicitly):
	AccountId,
	// We don't track any teleports of `Balances`.
	(),
>;

/// Means for transacting assets on this chain.
pub type AssetTransactors = FungibleTransactor;

/// This is the type we use to convert an (incoming) XCM origin into a local `Origin` instance,
/// ready for dispatching a transaction with XCM's `Transact`. There is an `OriginKind` that can
/// bias the kind of local `Origin` it will become.
pub type XcmOriginToTransactDispatchOrigin = (
	// Governance locations can gain root.
	LocationAsSuperuser<IsGovernanceLocation, RuntimeOrigin>,
	// Sovereign account converter; this attempts to derive an `AccountId` from the origin location
	// using `LocationToAccountId` and then turn that into the usual `Signed` origin. Useful for
	// foreign chains who want to have a local sovereign account on this chain that they control.
	SovereignSignedViaLocation<LocationToAccountId, RuntimeOrigin>,
	// Native converter for Relay-chain (Parent) location; will convert to a `Relay` origin when
	// recognized.
	RelayChainAsNative<RelayChainOrigin, RuntimeOrigin>,
	// Native converter for sibling Parachains; will convert to a `SiblingPara` origin when
	// recognized.
	SiblingParachainAsNative<cumulus_pallet_xcm::Origin, RuntimeOrigin>,
	// Native signed account converter; this just converts an `AccountId32` origin into a normal
	// `RuntimeOrigin::Signed` origin of the same 32-byte value.
	SignedAccountId32AsNative<RelayNetwork, RuntimeOrigin>,
	// XCM origins can be represented natively under the XCM pallet's `Xcm` origin.
	XcmPassthrough<RuntimeOrigin>,
);

pub struct ParentOrParentsPlurality;
impl Contains<Location> for ParentOrParentsPlurality {
	fn contains(location: &Location) -> bool {
		matches!(location.unpack(), (1, []) | (1, [Plurality { .. }]))
	}
}

parameter_types! {
	/// Sibling parachain IDs authorized to dispatch storage authorizations via
	/// XCM. Storage-backed so governance (root) can update via
	/// `system.set_storage` without a runtime upgrade.
	pub storage AllowedParachainIds: Vec<u32> = vec![1502, 5140];
	/// Sibling parachain IDs treated as governance origins. Storage-backed so
	/// governance (root) can update via `system.set_storage` without a runtime
	/// upgrade.
	pub storage GovernanceParachainIds: Vec<u32> = vec![1500];
}

/// Filter matching sibling parachain origins listed in [`AllowedParachainIds`]
/// — the parachains allowed to dispatch storage authorizations via XCM.
pub struct IsAuthorizerParachain;
impl Contains<Location> for IsAuthorizerParachain {
	fn contains(location: &Location) -> bool {
		match location.unpack() {
			(1, [Parachain(id)]) => AllowedParachainIds::get().contains(id),
			_ => false,
		}
	}
}

/// Filter matching sibling parachain origins listed in [`GovernanceParachainIds`].
pub struct IsGovernanceLocation;
impl Contains<Location> for IsGovernanceLocation {
	fn contains(location: &Location) -> bool {
		match location.unpack() {
			(1, [Parachain(id)]) => GovernanceParachainIds::get().contains(id),
			_ => false,
		}
	}
}

/// Matches the `Voice` of body `B` from any sibling parachain listed in
/// [`GovernanceParachainIds`] (multi-paraId variant of `IsVoiceOfBody`).
pub struct IsGovernanceVoiceOfBody<B>(core::marker::PhantomData<B>);
impl<B: frame_support::traits::Get<BodyId>> Contains<Location> for IsGovernanceVoiceOfBody<B> {
	fn contains(location: &Location) -> bool {
		match location.unpack() {
			(1, [Parachain(id), Plurality { id: body_id, part: BodyPart::Voice }])
				if *body_id == B::get() =>
				GovernanceParachainIds::get().contains(id),
			_ => false,
		}
	}
}

pub struct FellowsPlurality;
impl Contains<Location> for FellowsPlurality {
	fn contains(location: &Location) -> bool {
		matches!(
			location.unpack(),
			(1, [Parachain(COLLECTIVES_ID), Plurality { id: BodyId::Technical, .. }])
		)
	}
}

pub type Barrier = TrailingSetTopicAsId<(
	// Allow local users to buy weight credit.
	TakeWeightCredit,
	// Expected responses are OK.
	AllowKnownQueryResponses<PolkadotXcm>,
	WithComputedOrigin<
		(
			// If the message is one that immediately attempts to pay for execution, then
			// allow it.
			AllowTopLevelPaidExecutionFrom<Everything>,
			// Parent, its pluralities (i.e. governance bodies), Fellows plurality,
			// governance parachains, and authorizer parachains get free execution.
			AllowExplicitUnpaidExecutionFrom<(
				ParentOrParentsPlurality,
				FellowsPlurality,
				IsGovernanceLocation,
				IsAuthorizerParachain,
			)>,
			// Subscriptions for version tracking are OK.
			AllowSubscriptionsFrom<ParentRelayOrSiblingParachains>,
			// HRMP notifications from the relay chain are OK.
			AllowHrmpNotificationsFromRelayChain,
		),
		UniversalLocation,
		ConstU32<8>,
	>,
)>;

parameter_types! {
	pub TreasuryAccount: AccountId = TREASURY_PALLET_ID.into_account_truncating();
}

/// Locations that will not be charged fees in the executor, neither for execution nor delivery.
/// We only waive fees for system functions, which these locations represent.
pub type WaivedLocations = (
	Equals<RootLocation>,
	RelayOrOtherSystemParachains<AllSiblingSystemParachains, Runtime>,
	Equals<AssetHubLocation>,
);

/// Cases where a remote origin is accepted as trusted Teleporter for a given asset.
/// Trust the relay chain and other system parachains to teleport the relay chain native token.
pub type TrustedTeleporters = ConcreteAssetFromSystem<TokenRelayLocation>;

/// Defines origin aliasing rules for this chain.
///
/// - Allow any origin to alias into a child sub-location (equivalent to DescendOrigin),
/// - Allow same accounts to alias into each other across system chains,
/// - Allow AssetHub root to alias into anything,
/// - Allow origins explicitly authorized to alias into target location.
pub type TrustedAliasers = (
	AliasChildLocation,
	AliasAccountId32FromSiblingSystemChain,
	AliasOriginRootUsingFilter<AssetHubLocation, Everything>,
	AuthorizedAliasers<Runtime>,
);

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type RuntimeCall = RuntimeCall;
	type XcmSender = XcmRouter;
	type XcmEventEmitter = PolkadotXcm;
	type AssetTransactor = AssetTransactors;
	type OriginConverter = XcmOriginToTransactDispatchOrigin;
	type IsReserve = ();
	type IsTeleporter = TrustedTeleporters;
	type UniversalLocation = UniversalLocation;
	type Barrier = Barrier;
	type Weigher = WeightInfoBounds<
		crate::weights::xcm::BulletinPaseoXcmWeight<RuntimeCall>,
		RuntimeCall,
		MaxInstructions,
	>;
	type Trader = UsingComponents<
		WeightToFee,
		TokenRelayLocation,
		AccountId,
		Balances,
		ResolveTo<StakingPotAccountId<Runtime>, Balances>,
	>;
	type ResponseHandler = PolkadotXcm;
	type AssetTrap = PolkadotXcm;
	type SubscriptionService = PolkadotXcm;
	type PalletInstancesInfo = AllPalletsWithSystem;
	type MaxAssetsIntoHolding = MaxAssetsIntoHolding;
	type AssetLocker = ();
	type AssetExchanger = ();
	type FeeManager = XcmFeeManagerFromComponents<
		WaivedLocations,
		SendXcmFeeToAccount<Self::AssetTransactor, TreasuryAccount>,
	>;
	type MessageExporter = ();
	type UniversalAliases = Nothing;
	type CallDispatcher = RuntimeCall;
	type SafeCallFilter = EverythingBut<crate::storage::StorageCallInspector>;
	type Aliasers = TrustedAliasers;
	type TransactionalProcessor = FrameTransactionalProcessor;
	type HrmpNewChannelOpenRequestHandler = ();
	type HrmpChannelAcceptedHandler = ();
	type HrmpChannelClosingHandler = ();
	type XcmRecorder = PolkadotXcm;
}

/// Converts a local signed origin into an XCM location. Forms the basis for local origins
/// sending/executing XCMs.
pub type LocalOriginToLocation = SignedToAccountId32<RuntimeOrigin, AccountId, RelayNetwork>;

pub type PriceForParentDelivery =
	ExponentialPrice<FeeAssetId, BaseDeliveryFee, TransactionByteFee, ParachainSystem>;

/// The means for routing XCM messages which are not for local execution into the right message
/// queues.
pub type XcmRouter = WithUniqueTopic<(
	// Two routers - use UMP to communicate with the relay chain:
	cumulus_primitives_utility::ParentAsUmp<ParachainSystem, PolkadotXcm, PriceForParentDelivery>,
	// ..and XCMP to communicate with the sibling chains.
	XcmpQueue,
)>;

parameter_types! {
	pub const DepositPerItem: Balance = crate::deposit(1, 0);
	pub const DepositPerByte: Balance = crate::deposit(0, 1);
	pub const AuthorizeAliasHoldReason: RuntimeHoldReason = RuntimeHoldReason::PolkadotXcm(pallet_xcm::HoldReason::AuthorizeAlias);
}

impl pallet_xcm::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	// We want to disallow users sending (arbitrary) XCM programs from this chain.
	type SendXcmOrigin = EnsureXcmOrigin<RuntimeOrigin, ()>;
	type XcmRouter = XcmRouter;
	// We support local origins dispatching XCM executions.
	type ExecuteXcmOrigin = EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
	type XcmExecuteFilter = Everything;
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type XcmTeleportFilter = Everything;
	type XcmReserveTransferFilter = Nothing;
	type Weigher = WeightInfoBounds<
		crate::weights::xcm::BulletinPaseoXcmWeight<RuntimeCall>,
		RuntimeCall,
		MaxInstructions,
	>;
	type UniversalLocation = UniversalLocation;
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
	const VERSION_DISCOVERY_QUEUE_SIZE: u32 = 100;
	type AdvertisedXcmVersion = pallet_xcm::CurrentXcmVersion;
	type Currency = Balances;
	type CurrencyMatcher = ();
	type TrustedLockers = ();
	type SovereignAccountOf = LocationToAccountId;
	type MaxLockers = ConstU32<8>;
	type WeightInfo = crate::weights::pallet_xcm::WeightInfo<Runtime>;
	type AdminOrigin = EnsureRoot<AccountId>;
	type MaxRemoteLockConsumers = ConstU32<0>;
	type RemoteLockConsumerIdentifier = ();
	// xcm_executor::Config::Aliasers also uses pallet_xcm::AuthorizedAliasers.
	type AuthorizedAliasConsideration = HoldConsideration<
		AccountId,
		Balances,
		AuthorizeAliasHoldReason,
		LinearStoragePrice<DepositPerItem, DepositPerByte, Balance>,
	>;
}

impl cumulus_pallet_xcm::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type XcmExecutor = XcmExecutor<XcmConfig>;
}
