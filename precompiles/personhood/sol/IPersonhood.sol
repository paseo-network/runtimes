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

pragma solidity ^0.8.30;

/// @title IPersonhood - Proof of Personhood Precompile
/// @notice Query personhood status of an account.
/// @dev Available at address 0x000000000000000000000000000000000a010000.
///      The precompile reads from the alias-accounts pallet which stores per-context
///      alias mappings backed by ring membership proofs. Ring roots are received from
///      the People chain via XCM pub/sub.
///
/// Example usage:
///   IPersonhood personhood = IPersonhood(0x000000000000000000000000000000000a010000);
///   IPersonhood.PersonhoodInfo memory info = personhood.personhoodStatus(someAddress, bytes32("dotns"));
///   if (info.status == 2) { /* full person */ }
/// @custom:security-contact admin@parity.io
interface IPersonhood {
    /// @notice Personhood information for an account in a given context
    /// @param status The personhood verification tier
    /// @param contextAlias Context-specific 32-byte pseudonym derived from ring membership proof.
    ///        Unique per person per context, preventing cross-application linkability.
    ///        Zero when status is None.
    /// @dev status tiers are defined incrementally: 0=None, 1=Lite, 2=Full.
    ///      If we add more types in the future, the existing ones remain unchanged.
    struct PersonhoodInfo {
        uint8 status;
        bytes32 contextAlias;
    }

    /// @notice Inputs to `personhoodInfoByProof`.
    /// @param expectedStatus Personhood tier the proof is claimed to attest to: 1 = Lite,
    ///        2 = Full. Any other value is rejected and `false` is returned.
    /// @param proof SCALE-encoded ring-signature proof produced by the configured crypto.
    ///        Calldata exceeding the configured proof's max encoded length is rejected
    ///        without attempting to decode; `false` is returned.
    /// @param expectedAlias The 32-byte alias the proof must derive in the given context.
    ///        Returns `false` if the verified alias differs.
    /// @param ringIndex Index of the ring within the collection that the proof references.
    /// @param context A 32-byte application identifier (same semantics as `personhoodStatus`).
    /// @param revision Revision of the ring root the proof was created against.
    /// @param message Message bound into the proof. SECURITY: the precompile does not bind this
    ///        to `msg.sender`; if the calling contract needs the verification tied to a
    ///        specific account, it MUST embed that account into `message` at proof-generation
    ///        time (e.g. `keccak256(abi.encodePacked(msg.sender, nonce))`). Otherwise anyone
    ///        holding a copy of the proof and calldata can re-submit it.
    struct ProofVerificationRequest {
        uint8 expectedStatus;
        bytes proof;
        bytes32 expectedAlias;
        uint32 ringIndex;
        bytes32 context;
        uint32 revision;
        bytes message;
    }

    /// @notice Returns personhood info for an account within a specific application context
    /// @param account The address to query
    /// @param context A 32-byte application identifier. Each application picks a fixed constant
    ///        (e.g. `bytes32("dotns")`). The same person receives a different contextAlias in
    ///        each context, preventing cross-application linkability.
    /// @return info The personhood info struct. All fields are zero when the account has no
    ///         personhood.
    function personhoodStatus(address account, bytes32 context) external view returns (PersonhoodInfo memory info);

    /// @notice Verifies a ring-membership proof against a single collection and returns whether
    ///         the proof attests to the claimed personhood tier.
    /// @dev Pure verification: no state is mutated and no alias-to-account mapping is created.
    ///      The collection is selected by `request.expectedStatus` (1 = Lite, 2 = Full); only
    ///      that collection is verified. See `ProofVerificationRequest` for field-level docs,
    ///      including the replay-protection contract on `request.message`.
    /// @param request Bundled verification inputs.
    /// @return ok `true` on successful verification, `false` on any failure.
    function personhoodInfoByProof(ProofVerificationRequest calldata request) external view returns (bool ok);
}
