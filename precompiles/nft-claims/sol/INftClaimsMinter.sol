// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

/// @dev Collection-minter registration for NFT claims, at one fixed precompile address.
///
/// A Scarcity collection accepts deposit-free claim minting only after its owner registers
/// it with an item selection. The registration mutators dispatch as the caller, so only the
/// collection's current Scarcity owner can register or withdraw it; anyone else reverts.
interface INftClaimsMinter {
    /// @dev Register the caller's `collection` with pseudo-random item selection: each claim mints
    /// the item drawn from the credit modulo the collection's next item index. The draw reads the
    /// first four bytes of the credit as a little-endian number, so it is stable per credit but
    /// depends on that index. Define every item before claims open, because a later one raises the
    /// index and re-draws every unclaimed credit. Delete none, because the index never shrinks, so
    /// the credits drawing a deleted item fail while the rest keep working.
    function setRandomMinter(uint32 collection) external;

    /// @dev Register the caller's `collection` with `minter` picking the item per claim.
    /// `minter` must have contract code deployed, otherwise the call reverts; nothing proves
    /// the code implements `mint(uint32,bytes32) returns (uint32)`, so a wrong contract
    /// fails claims instead.
    function setContractMinter(uint32 collection, address minter) external;

    /// @dev Withdraw the caller's `collection` from claims. Nothing already claimed is undone.
    /// Withdrawing a collection that is not registered succeeds, but the caller must still be its
    /// current Scarcity owner and the collection must still exist, because those are checked
    /// before the registration is looked at. After an ownership handover the former owner
    /// therefore cannot withdraw the registration they made; only the new owner can.
    function clearMinter(uint32 collection) external;

    /// @dev The stored registration: `kind` 0 = unregistered, 1 = random, 2 = contract.
    /// `minter` is the picking contract for kind 2 and the zero address otherwise; `owner` is
    /// the account that registered it, or the zero address when unregistered. This reports
    /// what is stored, not whether a claim will mint: a claim also needs `owner` to still be
    /// the collection's current Scarcity owner, kind 2's `minter` to still carry code, and the
    /// drawn item to still exist.
    function collectionMinter(uint32 collection) external view returns (uint8 kind, address minter, address owner);
}
