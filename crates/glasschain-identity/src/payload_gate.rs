// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! The private-payload gate, proved in production form with Verus
//! (ADR-019/#176).
//!
//! A peer's org may receive private payloads only when a certificate verifier
//! is configured, the org was certificate-verified in this session, and the org
//! is a channel member (ADR-003, #86). The caller reduces its lookups to the
//! three booleans, so the gate lives beside channel membership but in its own
//! module: it is a node-level dissemination decision, not channel bookkeeping.
//!
//! The kernel takes primitives on purpose (the TOFU precedent): the specs stay
//! the decision table, and the hash lookups stay behind the seam.

use vstd::prelude::*;

verus! {

/// The private-payload gate: all three conditions must hold.
pub open spec fn spec_private_payload_allowed(
    verifier_present: bool,
    org_verified: bool,
    org_is_member: bool,
) -> bool {
    verifier_present && org_verified && org_is_member
}

/// Decide the gate from the extracted conditions.
#[must_use]
pub const fn private_payload_allowed(
    verifier_present: bool,
    org_verified: bool,
    org_is_member: bool,
) -> (allowed: bool)
    ensures
        allowed == spec_private_payload_allowed(verifier_present, org_verified, org_is_member),
{
    verifier_present && org_verified && org_is_member
}

/// Fail closed: any missing condition denies private payloads.
pub proof fn missing_condition_denies_private_payloads(
    verifier_present: bool,
    org_verified: bool,
    org_is_member: bool,
)
    ensures
        !verifier_present ==> !spec_private_payload_allowed(
            verifier_present,
            org_verified,
            org_is_member,
        ),
        !org_verified ==> !spec_private_payload_allowed(
            verifier_present,
            org_verified,
            org_is_member,
        ),
        !org_is_member ==> !spec_private_payload_allowed(
            verifier_present,
            org_verified,
            org_is_member,
        ),
{
}

} // verus!
