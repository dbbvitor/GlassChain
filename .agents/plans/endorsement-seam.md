# Plan — Endorsement provider seam and policy engine (ticket #37)

**Ticket:** [#37 Endorsement provider seam and policy engine](https://github.com/dbbvitor/GlassChain/issues/37)
**Spec:** ADR-008 (implementation handoff 1–2 + tests; handoff 3–4 are #45)

## Scope

1. **`glasschain-core/src/endorsement.rs`** — identity-neutral seam:
   - `Principal` (newtype), `PolicyExpression::{SignedBy, NOutOf}` with
     `and`/`or` builders that serialize to `NOutOf`; pure deterministic tree
     evaluation over a set of distinct principals; `validate()` enforcing
     non-empty principals, `required >= 1`, and `required <= rules.len()`
     (no allow-all).
   - `ScopedTarget { channel, contract, keys, collection }`,
     `ScopedPolicies { channel_default (mandatory), contract_default,
     collection_policy, key_policies }` with `applicable()` composition in
     precedence order — every layer must be satisfied, so a more specific
     policy can only add constraints.
   - `EndorsementRequest { target, payload, signers }`,
     `EndorserIdentity { claimed_principal, public_key, signature }`,
     `EndorsementResult { satisfied, distinct_principals, required }`.
   - `EndorsementProvider` trait (`evaluate` + `name`) in `providers.rs` —
     `glasschain-core` stays identity-free.
2. **`glasschain-identity/src/msp_policy.rs`** — `MspEndorsementProvider`:
   key→principal directory (`register`), ed25519 verification of the request
   payload, **forged-label rejection** (claimed principal must match the
   registered one), unknown-key rejection, invalid-signature skip, and
   **distinct-principal counting** (duplicates/replays never increase the
   count). `Identity::sign_bytes` added for request signing.
3. **Tests** — core: tree evaluation, builder serialization, validation rules,
   applicable() precedence incl. multi-key and collection layers, serde
   determinism. Identity: nested `NOutOf`, 2-of-2 across organizations,
   duplicate/replayed signatures, forged labels, unknown keys, multi-key
   targets. `PLUGIN_KIT.md` documents the new trait.

## Out of scope (ticket #45)

Committed-history policy metadata, commit-path invocation, same-block
policy-update rule, custody/PDC operation defaults, and the
`VerifyEndorsement` RPC.
