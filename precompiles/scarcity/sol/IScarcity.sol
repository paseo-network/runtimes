// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

/// @dev ERC-721 view of one Scarcity collection, plus the collection owner's admin surface.
///
/// Each collection is its own virtual contract: the collection id is encoded big-endian in
/// the first four bytes of the precompile address. The ERC-721 subset keeps the exact
/// standard signatures (and therefore selectors); the admin and Scarcity-specific read
/// functions below the events are this precompile's own surface.
///
/// The address range covers every id, so an address whose collection was never created or has
/// been deleted reverts on every function below rather than answering as an empty collection.
/// The per-function documentation describes live collections only.
///
/// Transfers move a token on its holder's own authority: the caller must be the holder.
/// Approvals are not supported, so `approve` and `setApprovalForAll` revert, `getApproved`
/// returns the zero address and `isApprovedForAll` returns false.
/// Note that due to ABI generation constraints, all interfaces are merged into a single
/// interface here.
interface IScarcityCollection {
    // ============================================================
    // IERC165
    // ============================================================

    /// @dev Returns true for the ERC-165 (0x01ffc9a7), ERC-721 (0x80ac58cd),
    /// ERC-721 Metadata (0x5b5e139f), ERC-5192 (0xb45a3c0e), ERC-2981 (0x2a55205a) and
    /// ERC-4906 (0x49064906) interface identifiers. Two more are deliberately not claimed,
    /// because an identifier covers every function of its interface: ERC-721 Enumerable
    /// (0x780e9d63) would need `totalSupply` and `tokenByIndex` beside {tokenOfOwnerByIndex},
    /// and ERC-173 (0x7f5828d0) would need `transferOwnership` beside {owner}. Both of those
    /// reads are served anyway, for tooling that calls them without asking first.
    function supportsInterface(bytes4 interfaceId) external view returns (bool);

    // ============================================================
    // IERC721
    // ============================================================

    /// @dev Emitted when `tokenId` is transferred from `from` to `to`.
    event Transfer(address indexed from, address indexed to, uint256 indexed tokenId);

    /// @dev Declared for ABI completeness and never emitted: {approve} always reverts, so no
    /// approval can come into being.
    event Approval(address indexed owner, address indexed approved, uint256 indexed tokenId);

    /// @dev Declared for ABI completeness and never emitted: {setApprovalForAll} always
    /// reverts, so no operator approval can come into being.
    event ApprovalForAll(address indexed owner, address indexed operator, bool approved);

    /// @dev Returns the number of tokens of this collection held by `owner`, always 0 or 1
    /// because a purse key holds at most one instance. Reverts for the zero address. Every path
    /// that gives a key an instance registers that key, so a live holder answers correctly
    /// whatever its balance. A holder whose account has since been reaped answers 0, because
    /// reaping drops the registration and its address cannot otherwise be resolved back. See
    /// {ownerOf}.
    function balanceOf(address owner) external view returns (uint256);

    /// @dev Returns the purse key holding `tokenId`. Reverts if `tokenId` is not a live
    /// instance of this collection. The address is stable and correct for every holder, unlike
    /// {balanceOf}, so prefer this to establish ownership.
    ///
    /// A purse key holds 32 bytes and an address holds 20, so an unregistered key is reported as
    /// a truncated hash of itself, and that hash resolves back to a different account. Occupying
    /// a key registers it, so the only holder this reaches is one whose account was reaped
    /// afterwards. Passing such an address to {mint}, {forceTransfer} or
    /// {nominateCollectionOwner} names an account no key can sign for; {forceTransfer} rejects
    /// the one case it can detect, where the address given is the instance's current holder.
    function ownerOf(uint256 tokenId) external view returns (address);

    /// @dev As {safeTransferFrom}. `data` is only ever forwarded to a receiver callback, which
    /// this precompile cannot yet make, so it is accepted and ignored.
    function safeTransferFrom(address from, address to, uint256 tokenId, bytes calldata data) external;

    /// @dev As {transferFrom}, and additionally reverts when `to` carries code. The acknowledgement
    /// {IERC721Receiver-onERC721Received} owes such a destination cannot be called yet, and
    /// skipping it would drop the only guarantee this variant adds over {transferFrom}. The
    /// refusal is decided after the move, so an invalid `tokenId` is reported ahead of it and the
    /// move is undone with the frame.
    function safeTransferFrom(address from, address to, uint256 tokenId) external;

    /// @dev Move `tokenId` to the empty purse key `to`. The caller must be `from` and `from`
    /// must be the current holder. Emits {Transfer}.
    function transferFrom(address from, address to, uint256 tokenId) external;

    /// @dev Always reverts: the purse model has no approval mechanism, so there is no
    /// per-token approval to grant. See {getApproved}.
    function approve(address to, uint256 tokenId) external;

    /// @dev Always reverts: the purse model has no approval mechanism, so there is no
    /// operator approval to grant. See {isApprovedForAll}.
    function setApprovalForAll(address operator, bool approved) external;

    /// @dev Always returns the zero address, because {approve} always reverts. Reverts if
    /// `tokenId` is not a live instance of this collection.
    function getApproved(uint256 tokenId) external view returns (address);

    /// @dev Always returns false, because {setApprovalForAll} always reverts. Never reverts,
    /// unlike {getApproved}, which rejects an unknown token before answering.
    function isApprovedForAll(address owner, address operator) external view returns (bool);

    // ============================================================
    // IERC721Metadata
    // ============================================================

    /// @dev Collection metadata value for the reserved key `name`, or the empty string. The
    /// pallet stores bytes, so a value that is not UTF-8 decodes with replacement characters
    /// rather than failing the call.
    function name() external view returns (string memory);

    /// @dev Collection metadata value for the reserved key `symbol`, or the empty string.
    /// Decodes like {name}.
    function symbol() external view returns (string memory);

    /// @dev Metadata value for the reserved key `tokenURI`, resolved instance, then item,
    /// then collection scope, or the empty string. Decodes like {name}. Reverts if `tokenId`
    /// is not a live instance of this collection.
    function tokenURI(uint256 tokenId) external view returns (string memory);

    // ============================================================
    // IERC721Enumerable (partial)
    // ============================================================

    /// @dev The token `owner` holds, for `index` 0 only. A purse key holds at most one instance,
    /// so every other index is out of range and reverts, as does an `owner` holding nothing and
    /// the zero address. Resolving `owner` has the same limitation as {balanceOf}: a key that is
    /// not registered reads as holding nothing.
    ///
    /// Served because wallets call it opportunistically. It does not make this collection
    /// ERC-721 Enumerable, and {supportsInterface} does not claim that identifier.
    function tokenOfOwnerByIndex(address owner, uint256 index) external view returns (uint256);

    // ============================================================
    // IERC2981
    // ============================================================

    /// @dev Royalty recipient and amount owed on a sale of `tokenId` at `salePrice`.
    /// Read from the reserved metadata keys `royaltyReceiver` (20 raw address bytes) and
    /// `royaltyBasisPoints` (a SCALE-encoded uint128, at most 10000), each resolved item, then
    /// collection scope. Answers the zero address and zero amount whenever those keys do not
    /// describe a usable royalty: unset, a receiver that is not an address or is the zero
    /// address, points that do not decode, or a share above 10000, so a misconfigured collection
    /// never blocks a settling marketplace. Reverts if `tokenId` is not a live instance of this
    /// collection, and if `salePrice` scaled by the basis points leaves the `uint256` range,
    /// because a wrapped amount would quote a royalty unrelated to the sale.
    function royaltyInfo(uint256 tokenId, uint256 salePrice) external view returns (address receiver, uint256 royaltyAmount);

    // ============================================================
    // IERC7572
    // ============================================================

    /// @dev Emitted when a write to the reserved collection key `contractURI` changes what
    /// {contractURI} returns. Carries no arguments, per ERC-7572: a consumer refetches the
    /// document rather than reading the new value from the log.
    event ContractURIUpdated();

    /// @dev Collection metadata value for the reserved key `contractURI`, or the empty string.
    /// Marketplaces read collection-level name, description and imagery from the document it
    /// points at. {setCollectionMetadata} and {removeCollectionMetadata} emit
    /// {ContractURIUpdated} when they touch that key.
    function contractURI() external view returns (string memory);

    // ============================================================
    // IERC5192
    // ============================================================

    /// @dev Emitted when {mint} creates a token whose item is soulbound.
    ///
    /// Transferability is fixed when the item is defined, so a token's status never changes
    /// after this and no later emission is possible.
    event Locked(uint256 tokenId);

    /// @dev Emitted when {mint} creates a token whose item is transferable. Final, like
    /// {Locked}.
    event Unlocked(uint256 tokenId);

    /// @dev True when `tokenId` cannot be moved on its holder's authority. Reverts if
    /// `tokenId` is not a live instance of this collection. The collection owner's
    /// {forceTransfer} and {forceBurn} are a separate authority and ignore this, so a locked
    /// token can still be moved or destroyed by the owner: locking binds the holder, not the
    /// issuer, which keeps a remedy for a misdirected mint.
    function locked(uint256 tokenId) external view returns (bool);

    // ============================================================
    // IERC4906
    // ============================================================

    /// @dev Emitted when a write changes what {tokenURI} returns for one instance.
    event MetadataUpdate(uint256 tokenId);

    /// @dev Emitted when a write changes what {tokenURI} returns for more than one instance,
    /// over the inclusive token-id range `fromTokenId` to `toTokenId`.
    ///
    /// A collection- or item-scope write resolves into an unbounded set of instances that cannot
    /// be enumerated on chain, so the range is the whole `uint256` space. Scope comes from the
    /// log's source address instead, which names exactly one collection.
    ///
    /// ERC-4906 names these arguments with leading underscores. Argument names are not part of an
    /// event signature, so the topic hash is the standard one either way.
    event BatchMetadataUpdate(uint256 fromTokenId, uint256 toTokenId);

    // ============================================================
    // IERC173
    // ============================================================

    /// @dev Emitted when {claimCollectionOwnership} moves ownership. Nomination emits nothing,
    /// because it moves no authority.
    event OwnershipTransferred(address indexed previousOwner, address indexed newOwner);

    /// @dev The collection owner, identical to {collectionOwner}.
    ///
    /// Served under ERC-173's name because tooling calls it, but ERC-173 is not claimed: its
    /// identifier covers `transferOwnership` too, and that call cannot exist here. A handover
    /// carries the collection's storage deposit, so the successor has to accept it and be able
    /// to fund it, which is what {nominateCollectionOwner} and {claimCollectionOwnership} are
    /// for. A one-call transfer would charge an account that never agreed to pay.
    function owner() external view returns (address);

    // ============================================================
    // Scarcity events
    // ============================================================

    /// @dev Emitted when {deleteItem} removes an item definition. The index is never reused.
    event ItemDeleted(uint32 indexed item);

    /// @dev Emitted when {deleteCollection} removes this collection. Every function on this
    /// address reverts afterwards, and the id is never reused.
    event CollectionDeleted();

    // ============================================================
    // Scarcity admin (collection owner only)
    // ============================================================

    /// @dev Define a new immutable item in this collection with shared metadata defaults.
    /// `keys` and `values` are parallel arrays, reserved keys checked as in
    /// {setCollectionMetadata}. `soulbound` binds every instance minted from this item to the
    /// purse key it is minted into, and is fixed here. Returns the new item index.
    function defineItem(bool soulbound, bytes[] calldata keys, bytes[] calldata values) external returns (uint32);

    /// @dev Mint one instance of `item` into the empty purse key `to` with instance-level
    /// metadata overrides, reserved keys checked as in {setCollectionMetadata}. Returns the
    /// permanent token id, the same value carried by the {Transfer} this emits from the zero
    /// address.
    ///
    /// An address {ownerOf} reported for a holder whose account was reaped resolves to the
    /// fallback account, and this call cannot tell that address from an ordinary one. The
    /// instance lands on that fallback rather than on the purse key, so the address answers for
    /// two holders while {balanceOf} counts only the fallback. No key signs for it, so only the
    /// collection owner moves it again, through {forceTransfer} or {forceBurn}.
    function mint(uint32 item, address to, bytes[] calldata keys, bytes[] calldata values) external returns (uint256);

    /// @dev Move a live instance of this collection to the empty purse key `to`, on the
    /// collection owner's authority. Emits {Transfer}.
    ///
    /// Reverts if `to` is the address {ownerOf} reports for this token's own holder, which
    /// would otherwise move the instance away from a holder that asked for nothing. Any other
    /// address of a reaped holder strands the instance as described on {mint}.
    function forceTransfer(uint256 tokenId, address to) external;

    /// @dev Permanently remove a live instance of this collection, on the collection
    /// owner's authority. Emits {Transfer} to the zero address.
    function forceBurn(uint256 tokenId) external;

    /// @dev Set or overwrite the collection-scope metadata default under `key`. Reverts when
    /// `key` is one of the reserved keys `name`, `symbol`, `tokenURI` or `contractURI` and
    /// `value` is not valid UTF-8, so a mistake fails here rather than reading back as
    /// replacement characters.
    ///
    /// Emits {BatchMetadataUpdate} for the key `tokenURI` and {ContractURIUpdated} for the key
    /// `contractURI`. Every other key emits nothing, `name`, `symbol` and the ERC-2981 royalty
    /// keys included: a standard read reflects several of them, but no standard defines an event
    /// announcing the change, and ERC-4906 is scoped to what {tokenURI} returns.
    function setCollectionMetadata(bytes calldata key, bytes calldata value) external;

    /// @dev Remove the collection-scope metadata default under `key`. Removing an absent
    /// key is a successful no-op, which still emits: the absent entry may have been shadowing
    /// nothing, and a removal changes what {tokenURI} resolves for every instance that fell
    /// back to it. Emits as {setCollectionMetadata}.
    function removeCollectionMetadata(bytes calldata key) external;

    /// @dev Set or overwrite the item-scope metadata default of `item` under `key`. The
    /// value is shared by every instance minted from `item`. Reserved keys are checked as in
    /// {setCollectionMetadata}.
    ///
    /// Emits {BatchMetadataUpdate} for the key `tokenURI`. The affected instances are exactly
    /// those minted from `item`, which cannot be enumerated, so the range is unbounded.
    function setItemMetadata(uint32 item, bytes calldata key, bytes calldata value) external;

    /// @dev Remove the item-scope metadata default of `item` under `key`. Removing an
    /// absent key is a successful no-op. Emits as {setItemMetadata}.
    function removeItemMetadata(uint32 item, bytes calldata key) external;

    /// @dev Set or overwrite the instance-scope metadata override of `tokenId` under `key`.
    /// Reserved keys are checked as in {setCollectionMetadata}. Reverts if `tokenId` is not a
    /// live instance of this collection.
    ///
    /// Emits {MetadataUpdate} for the key `tokenURI`, naming just this token.
    function setInstanceMetadata(uint256 tokenId, bytes calldata key, bytes calldata value) external;

    /// @dev Remove the instance-scope metadata override of `tokenId` under `key`. Removing
    /// an absent key is a successful no-op. Reverts if `tokenId` is not a live instance of
    /// this collection. Emits as {setInstanceMetadata}.
    function removeInstanceMetadata(uint256 tokenId, bytes calldata key) external;

    /// @dev Nominate `successor` to claim ownership of this collection. Nomination alone
    /// changes no authority; the successor must call {claimCollectionOwnership}. Reverts
    /// for the zero address; use {clearCollectionOwnerNomination} to withdraw a nomination.
    ///
    /// An address {ownerOf} reported for a holder whose account was reaped names an account
    /// that can never claim. Nominating again replaces it, so nothing is lost.
    function nominateCollectionOwner(address successor) external;

    /// @dev Clear the pending ownership nomination.
    function clearCollectionOwnerNomination() external;

    /// @dev Claim ownership of this collection. Only the nominated successor may call this,
    /// so it is the one admin function not gated on the current owner. The caller assumes
    /// the collection's aggregate storage deposit and the former owner's deposit is released.
    /// Emits {OwnershipTransferred}.
    ///
    /// A handover does not carry the collection's claim-minting registration with it. That
    /// registration records the owner who made it, so claims stop the moment ownership moves,
    /// and only the new owner can restore or withdraw it. The registration functions live on a
    /// different precompile, at the fixed claims-minter address rather than this one.
    function claimCollectionOwnership() external;

    /// @dev Delete an item definition. Every live instance of `item` must be burnt and
    /// every item-scope metadata entry removed first; deleted item indices are never reused.
    /// Emits {ItemDeleted}.
    ///
    /// Deleting an item does not lower the collection's next item index, so a claim-minting
    /// registration that draws items by that index can draw a deleted one and fail, and no later
    /// definition closes that hole. Defining a replacement raises the index, which redraws the
    /// item for every unclaimed credit: the failures move to a different set of credits rather
    /// than ending.
    function deleteItem(uint32 item) external;

    /// @dev Delete this collection and release its remaining deposit. Every item definition
    /// must be deleted and every collection-scope metadata entry removed first; the
    /// collection id is never reused. Emits {CollectionDeleted}.
    function deleteCollection() external;

    // ============================================================
    // Scarcity reads
    // ============================================================

    /// @dev The collection owner's address.
    function collectionOwner() external view returns (address);

    /// @dev The account nominated to claim this collection, or the zero address when none.
    function pendingCollectionOwner() external view returns (address);

    /// @dev The collection's aggregate storage deposit, charged to the current owner and
    /// assumed by a successor on {claimCollectionOwnership}.
    function collectionOwnerDeposit() external view returns (uint256);

    /// @dev True when a collection-scope entry exists under `key`, even with an empty
    /// value.
    function hasCollectionMetadata(bytes calldata key) external view returns (bool);

    /// @dev True when an item-scope entry of `item` exists under `key`. Unlike
    /// {itemMetadata}, this does not fall back to the collection scope.
    function hasItemMetadata(uint32 item, bytes calldata key) external view returns (bool);

    /// @dev True when an instance-scope entry of `tokenId` exists under `key`. Unlike
    /// {instanceMetadata}, this does not fall back to the item or collection scope.
    /// Reverts if `tokenId` is not a live instance of this collection.
    function hasInstanceMetadata(uint256 tokenId, bytes calldata key) external view returns (bool);

    /// @dev Minted-ever and currently-live instance counts of `item`. Reverts if the item
    /// definition does not exist.
    function itemSupply(uint32 item) external view returns (uint32 supply, uint32 liveSupply);

    /// @dev Item index, mint time, last move time and state nonce of a live instance of
    /// this collection. Reverts if `tokenId` is not a live instance of this collection.
    function instanceInfo(uint256 tokenId) external view returns (uint32 item, uint64 mintedAt, uint64 lastMoved, uint64 stateNonce);

    /// @dev Collection-scope metadata value for `key`, or empty bytes when unset.
    function collectionMetadata(bytes calldata key) external view returns (bytes memory);

    /// @dev Metadata value for `key` resolved item, then collection scope, or empty bytes
    /// when unset.
    function itemMetadata(uint32 item, bytes calldata key) external view returns (bytes memory);

    /// @dev Metadata value for `key` resolved instance, then item, then collection scope,
    /// or empty bytes when unset.
    function instanceMetadata(uint256 tokenId, bytes calldata key) external view returns (bytes memory);
}

/// @dev Acknowledgement a contract destination of {IScarcityCollection-safeTransferFrom} answers.
///
/// Declared for the call that variant will make once `pallet-revive` exports the reentrancy
/// argument of its message-call primitive. Contract destinations are refused until then, so
/// nothing calls this yet.
interface IERC721Receiver {
    /// @dev Handle receipt of `tokenId`, already held by this contract when called. Return this
    /// function's own selector to keep the token; any other return value or a revert refuses it
    /// and undoes the transfer.
    function onERC721Received(address operator, address from, uint256 tokenId, bytes calldata data) external returns (bytes4);
}

/// @dev Factory surface of the Scarcity precompile, at its own fixed address.
///
/// Collection creation cannot live on the per-collection addresses because the collection
/// id does not exist until creation allocates it.
interface IScarcityFactory {
    /// @dev Emitted when a collection is created. `collection` is the id encoded into the
    /// collection's own precompile address.
    event CollectionCreated(uint32 indexed collection, address indexed owner);

    /// @dev Create a new collection owned by the caller. Returns the new collection id.
    function createCollection() external returns (uint32);
}
