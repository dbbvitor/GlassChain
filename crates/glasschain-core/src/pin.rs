// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! Durable identity-pin (TOFU) transition, proved in production form with
//! Verus (ADR-019: Verus first on zero-trust).
//!
//! The network's `PeerRegistry` records a peer's identity the first time it
//! connects and verifies every later connection against that record. The
//! accept/reject/rotate decision is the trust boundary; it is extracted here
//! and proved:
//!
//! * a poisoned (unreadable persisted) pin always rejects;
//! * a changed node id or organization always rejects;
//! * a changed certificate fingerprint only rotates with a proof that
//!   verifies under the **pinned** identity key — never merely because a new
//!   key was presented;
//! * an unchanged fingerprint is `Known`, never a re-key.
//!
//! The ed25519 check itself stays outside the proof: the caller reduces it to
//! [`RotationProof`]. Signature verification is a primitive the [`decide`]
//! proof is allowed to assume, matching the zero-trust policy for
//! `ed25519-dalek`/`ring` in ADR-019.

use vstd::prelude::*;

verus! {

/// The pinned identity fields of one peer address.
pub struct PinState<'a> {
    /// The pinned node id.
    pub node_id: &'a str,
    /// The pinned transport certificate fingerprint.
    pub cert_fingerprint: &'a str,
    /// The pinned organization.
    pub org: &'a str,
    /// The pinned identity public key, when first contact carried one.
    pub public_key: Option<&'a [u8]>,
}

/// The identity fields one `Hello` claims.
pub struct IdentityClaim<'a> {
    /// The claimed node id.
    pub node_id: &'a str,
    /// The claimed transport certificate fingerprint.
    pub cert_fingerprint: &'a str,
    /// The claimed organization.
    pub org: &'a str,
    /// The claimed identity public key, when the Hello carries one.
    pub public_key: Option<&'a [u8]>,
}

/// Whether a fingerprint change comes with a valid signed rotation.
///
/// The caller decides this by checking the proof under the **pinned** key
/// (`glasschain_identity::verify_ed25519`); this module never sees the bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationProof {
    /// No proof was presented.
    Missing,
    /// A proof was presented and does not verify under the pinned key.
    Invalid,
    /// A proof was presented and verifies under the pinned key.
    Valid,
}

/// Why a claim was rejected; the caller formats the operator-facing message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// The persisted pin for this address is unreadable (fail closed).
    Poisoned,
    /// The claimed node id differs from the pin.
    NodeIdChanged,
    /// The claimed organization differs from the pin.
    OrgChanged,
    /// The fingerprint changed but no pinned identity key can verify a
    /// rotation.
    NoPinnedKey,
    /// The fingerprint changed and no rotation proof was presented.
    MissingProof,
    /// The fingerprint changed and the presented rotation proof is invalid.
    InvalidProof,
}

/// The decision for one Hello.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinDecision {
    /// No pin exists: first contact records the claim.
    New,
    /// The identity is unchanged: session evidence may be refreshed.
    Known,
    /// The fingerprint changed under a valid rotation proof.
    Rotated,
    /// Rejected; the caller must leave the pin untouched.
    Rejected(RejectReason),
}

/// Spec-level `str` equality: the view comparison the decision model uses.
pub open spec fn spec_str_eq(left: &str, right: &str) -> bool {
    left@ == right@
}

/// `str` equality with the view-equality spec, usable from both exec code and
/// specs (`when_used_as_spec`), so the model and the shipped code compare with
/// one relation.
#[must_use]
#[verifier::when_used_as_spec(spec_str_eq)]
pub fn str_eq(left: &str, right: &str) -> (equal: bool)
    ensures equal == spec_str_eq(left, right),
{
    *left == *right
}

/// The specification of [`decide`], in the same case order.
///
/// The exec function is proved equal to this model, so every lemma below is a
/// statement about the shipped decision.
pub open spec fn spec_decide(
    existing: Option<&PinState>,
    poisoned: bool,
    claim: &IdentityClaim,
    rotation_proof: RotationProof,
) -> PinDecision {
    if poisoned {
        PinDecision::Rejected(RejectReason::Poisoned)
    } else {
        match existing {
            None => PinDecision::New,
            Some(pinned) => {
                if !str_eq(pinned.node_id, claim.node_id) {
                    PinDecision::Rejected(RejectReason::NodeIdChanged)
                } else if !str_eq(pinned.org, claim.org) {
                    PinDecision::Rejected(RejectReason::OrgChanged)
                } else if str_eq(pinned.cert_fingerprint, claim.cert_fingerprint) {
                    PinDecision::Known
                } else if pinned.public_key.is_none() {
                    PinDecision::Rejected(RejectReason::NoPinnedKey)
                } else {
                    match rotation_proof {
                        RotationProof::Missing => PinDecision::Rejected(
                            RejectReason::MissingProof,
                        ),
                        RotationProof::Invalid => PinDecision::Rejected(
                            RejectReason::InvalidProof,
                        ),
                        RotationProof::Valid => PinDecision::Rotated,
                    }
                }
            },
        }
    }
}

/// Decide a Hello's identity claim against the pinned state.
///
/// * No pin → [`PinDecision::New`] (the TOFU first-contact rule; the caller
///   records the claim verbatim).
/// * Same node id, organization and fingerprint → [`PinDecision::Known`].
/// * Changed fingerprint with a proof that verifies under the pinned key →
///   [`PinDecision::Rotated`]; the caller updates the fingerprint and may
///   adopt a newly presented key.
/// * Anything else, or a poisoned pin → [`PinDecision::Rejected`].
#[must_use]
pub fn decide(
    existing: Option<&PinState>,
    poisoned: bool,
    claim: &IdentityClaim,
    rotation_proof: RotationProof,
) -> (decision: PinDecision)
    ensures decision == spec_decide(existing, poisoned, claim, rotation_proof),
{
    if poisoned {
        return PinDecision::Rejected(RejectReason::Poisoned);
    }
    let Some(pinned) = existing else {
        return PinDecision::New;
    };
    if !str_eq(pinned.node_id, claim.node_id) {
        return PinDecision::Rejected(RejectReason::NodeIdChanged);
    }
    if !str_eq(pinned.org, claim.org) {
        return PinDecision::Rejected(RejectReason::OrgChanged);
    }
    if str_eq(pinned.cert_fingerprint, claim.cert_fingerprint) {
        return PinDecision::Known;
    }
    if pinned.public_key.is_none() {
        return PinDecision::Rejected(RejectReason::NoPinnedKey);
    }
    match rotation_proof {
        RotationProof::Missing => PinDecision::Rejected(RejectReason::MissingProof),
        RotationProof::Invalid => PinDecision::Rejected(RejectReason::InvalidProof),
        RotationProof::Valid => PinDecision::Rotated,
    }
}

/// A poisoned pin rejects unconditionally — a corrupt trust record can never
/// re-pin or rotate an address.
pub proof fn poisoned_always_rejects(
    existing: Option<&PinState>,
    claim: &IdentityClaim,
    rotation_proof: RotationProof,
)
    ensures spec_decide(existing, true, claim, rotation_proof) == PinDecision::Rejected(
        RejectReason::Poisoned,
    ),
{
}

/// An unchanged fingerprint is never a re-key: `Known` implies the pinned
/// fingerprint equals the claim's.
pub proof fn known_keeps_the_fingerprint(
    existing: &PinState,
    claim: &IdentityClaim,
    rotation_proof: RotationProof,
)
    ensures
        spec_decide(Some(existing), false, claim, rotation_proof) == PinDecision::Known
            ==> spec_str_eq(existing.cert_fingerprint, claim.cert_fingerprint),
{
}

/// A rotation requires a proof that verifies under the pinned key, and a
/// fingerprint that actually changed — the zero-trust headline.
pub proof fn rotation_requires_a_valid_proof(
    existing: &PinState,
    claim: &IdentityClaim,
    rotation_proof: RotationProof,
)
    ensures
        spec_decide(Some(existing), false, claim, rotation_proof) == PinDecision::Rotated
            ==> rotation_proof == RotationProof::Valid,
        spec_decide(Some(existing), false, claim, rotation_proof) == PinDecision::Rotated
            ==> !spec_str_eq(existing.cert_fingerprint, claim.cert_fingerprint),
{
}

/// A changed fingerprint is only ever `Rotated` with a valid proof: every
/// other changed-fingerprint outcome is a rejection.
pub proof fn fingerprint_change_needs_proof(
    existing: &PinState,
    claim: &IdentityClaim,
    rotation_proof: RotationProof,
)
    requires
        spec_str_eq(existing.node_id, claim.node_id),
        spec_str_eq(existing.org, claim.org),
        !spec_str_eq(existing.cert_fingerprint, claim.cert_fingerprint),
        existing.public_key.is_some(),
    ensures
        rotation_proof == RotationProof::Valid <==> spec_decide(
            Some(existing),
            false,
            claim,
            rotation_proof,
        ) == PinDecision::Rotated,
        rotation_proof != RotationProof::Valid ==> spec_decide(
            Some(existing),
            false,
            claim,
            rotation_proof,
        ) is Rejected,
{
}

} // verus!
