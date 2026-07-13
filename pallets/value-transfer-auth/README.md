# pallet-value-transfer-auth

A Substrate transaction extension that requires an Ed25519 signature before selected runtime calls or XCM asset operations may move a runtime-configured protected asset.

## Purpose

This crate provides three pieces.

1. **`AuthorizeValueTransfer<T, P>` transaction extension** — Verifies an Ed25519 signature over the transaction's `inherited_implication` and, on success, clears a global block flag (`block_flag::unblock`) for the duration of dispatch. `post_dispatch_details` re-sets the flag. Generic over runtime `T` and `Get<sp_core::ed25519::Public>` parameter `P`.

2. **`BlockValueTransfersWhenFlagSet<ValueTransfers>` call filter** — A `Contains<RuntimeCall>` adapter wired into the runtime's `frame_system::Config::BaseCallFilter`. `ValueTransfers` is any `Contains<RuntimeCall>` describing the value-transfer call set. When the global flag is set (default), those calls are rejected at every `RuntimeCall` dispatch site — top-level, XCM `Transact`, batches, proxies, scheduler — uniformly and early (before any storage access).

3. **`ProtectedAssetTransactor<Inner, ProtectedAssetLocation, TrustedSiblings>` XCM asset chokepoint** — Wraps an `xcm_executor::traits::TransactAsset` impl (typically `FungiblesAdapter`). Rejects `withdraw_asset` / `internal_transfer_asset` / `transfer_asset` / `mint_asset` / `deposit_asset` whenever the concrete `Asset` matches `ProtectedAssetLocation` and the block flag is set, with one exception: `deposit_asset` and `mint_asset` allow inbound deposits and mints from sibling chains in the `TrustedSiblings` `Contains<Location>` allowlist. This lets legitimate cross-chain transfers between sibling parachains settle without a local signature on the receiving chain.

A fourth piece, `RestrictProtectedAssetErc20`, lives in each runtime's source (see `runtimes/next-asset-hub-paseo/src/protected_asset_erc20_guard.rs`) and applies the same flag-gated rejection to the ERC20 precompile entry point used by `pallet-revive` contracts. It is the contracts-surface twin of `ProtectedAssetTransactor`.

## (0,0) position invariant

The `AuthorizeValueTransfer` extension **must** occupy the first slot of the inner "Origin modifiers" tuple in both runtimes' `TransactionExtension` type alias.

**Why:** An extension's `inherited_implication` covers the explicit + implicit data of every *subsequent* extension in the tuple, not the preceding ones. Placing `AuthorizeValueTransfer` first means its signature is bound to the call plus every other extension's contribution (nonce, era, genesis, fee tip, and on `next-people-paseo` also `VerifySignature`'s `{ signature, account }`). Move it later and earlier extensions silently drop out of the signed payload, enabling replay.

## Value-transfer classifier per runtime

Each runtime supplies its own concrete `Contains<RuntimeCall>` describing the set of calls that can move its configured guarded asset. No custom trait is required — the standard FRAME `frame_support::traits::Contains` is used directly.

The matcher is a pure leaf classifier — it does **not** recurse into Utility/Proxy/Multisig wrappers, because nested dispatch is gated automatically by `BaseCallFilter`.

Example usage on some system parachain flavors like Asset Hub and People Chain, which perform value transfers:

- **Asset Hub-style runtime**: Matches `pallet-assets` calls targeting the configured asset ID and `pallet-asset-conversion` value-bearing calls (`add_liquidity`, `remove_liquidity`, `swap_*`) whose asset path includes the configured XCM `Location`. `create_pool` and `touch` are setup-only and not gated.
- **People Chain-style runtime**: Matches `pallet-assets` calls targeting the configured XCM `Location` plus all `pallet-coinage` calls, because Coinage can move its configured underlying asset.

`Balances` (native token, DOT/PAS) and `PolkadotXcm` are intentionally NOT in the value-transfer set. Native transfers are not gated. XCM paths — `pallet_xcm::execute`, `pallet_xcm::send`, `transfer_assets`, incoming XCMP messages, `Transact` — are not classified at the call-filter layer because the concrete asset identity is inside the XCM message body. Instead they are caught one layer deeper at the executor's `AssetTransactor` by `ProtectedAssetTransactor`, which compares concrete `Asset` instances with `ProtectedAssetLocation` at every `withdraw_asset` / `deposit_asset` / `mint_asset` / `transfer_asset` boundary.

If a new pallet is added that can move a configured protected asset through `RuntimeCall` without going through `pallet-assets` or XCM, register it in the relevant runtime's `value_transfer_filter.rs` module. If it moves that asset through the `fungibles` trait surface and therefore the `TransactAsset` impl wrapping it, no filter change is needed — `ProtectedAssetTransactor` catches it automatically.

## Offline signers compute the signature

Offline signers compute the `AuthorizeValueTransfer` signature as follows:

1. Construct the full transaction, including all extensions in their final order.
2. Build the implication value FRAME would pass to slot (0,0) of the extension tuple. Per `sp_runtime::traits::ImplicationParts` (sdk 47), the shape is `ImplicationParts { base, explicit, implicit }` where:
   - `base` is `TxBaseImplication((extension_version_u8, &call))` — the `extension_version` is `0u8` for the v0 transaction-extension format.
   - `explicit` is the tuple of every transaction extension instance AFTER slot (0,0), in order (the inner Origin-modifiers tuple's slots 1..N plus the outer tuple's slots 1..M).
   - `implicit` is the tuple of each subsequent extension's `Implicit` value, in the same order.
3. Call `indiv_pallet_value_transfer_auth::extension::payload_hash(&inherited_implication)` to obtain the blake2_256 hash of the SCALE-encoded `ImplicationParts`.
4. Sign the hash with the authorization Ed25519 private key.
5. Construct the `AuthorizeValueTransfer` extension via the public tuple-struct fields: `AuthorizeValueTransfer::<T, P>(Some(signature), core::marker::PhantomData)`. The struct also implements `Default`, so a no-signature instance is `AuthorizeValueTransfer::<T, P>::default()`.

`AuthorizeValueTransfer` itself has `Implicit = ()`, so it does not appear in either the `explicit` or `implicit` portions of its own implication. The canonical reference implementation of the encoding (including the test helper that builds the `ImplicationParts` value identically to what FRAME composes at runtime) lives in `pallets/value-transfer-auth/src/tests/extension.rs` and in `runtimes/next-people-paseo/src/integration_tests/mod.rs`'s `finalize_uxt` helper. Match those byte-for-byte; any deviation will produce a signature that FRAME rejects with `InvalidTransaction::BadProof`.

`payload_hash` is re-exported from the crate root:

```rust
pub use extension::{AuthorizeValueTransfer, payload_hash};
```

External signers can import it directly:

```rust
use indiv_pallet_value_transfer_auth::payload_hash;
```

## Test pubkey override

The extension is generic over a `Get<sp_core::ed25519::Public>` type parameter, so tests supply a `parameter_types!` block with a test pubkey:

```rust
parameter_types! {
    pub TestAuthorizationPubkey: sp_core::ed25519::Public = test_keypair().1;
}

type Ext = AuthorizeValueTransfer<MockRuntime, TestAuthorizationPubkey>;
```

The `test_keypair()` helper returns a deterministic Ed25519 keypair seeded with `[0x42; 32]`. See `pallets/value-transfer-auth/src/mock.rs` for the pattern.

In integration tests, construct `ImplicationParts` with different implicit fields to verify that the signature hash changes:

```rust
use sp_runtime::traits::ImplicationParts;

let implication = ImplicationParts {
    base: (0u8, &call),
    explicit: (),
    implicit: 5u64,  // Simulates CheckNonce
};
let hash = payload_hash(&implication);
```

## transaction_version bumped (migration notes)

Both runtimes have incremented their `transaction_version` in the `VERSION` block:

- **`next-asset-hub-paseo`**: bumped from 15 to 16
- **`next-people-paseo`**: bumped from 1 to 2

This is a **metadata change only** — no on-chain storage migration is required. However, wallets and indexers must refresh their cached metadata after the upgrade. Transactions signed with the old metadata will fail because the extension tuple has changed shape.

## Open TODOs

### Protected asset identity

The reference runtime configuration for the guarded asset lives in `paseo-support/paseo-runtime-constants/src/lib.rs` under the `protected_asset` module, exposed as:

- `PROTECTED_ASSET_ID: u32` — the `pallet-assets` asset ID used by Asset Hub-style runtimes (`AssetId = u32`).
- `ProtectedAssetLocation: Location` — the XCM `Location` view used by People Chain-style runtimes (`AssetId = Location`) and by `ProtectedAssetTransactor`.

### Authorization pubkey

The authorization Ed25519 public key is defined in `paseo-support/paseo-runtime-constants/src/lib.rs` as `VALUE_TRANSFER_AUTHORIZATION_PUBKEY_BYTES` and exposed to both runtimes through the `ValueTransferAuthorizationPubkey` parameter type. Rotating the key requires a runtime upgrade.

Tests can override the embedded value per-thread via `paseo_runtime_constants::auth_keys::set_value_transfer_authorization_pubkey_override(Some(pubkey))`. The override is `#[cfg(feature = "std")]`-only and never reaches wasm.
