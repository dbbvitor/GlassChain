# ADR-011 — Federation trust store

**Status:** Accepted
**Date:** 2026-09-03
**Decision owner:** project owner
**Relates to:** §1.1 (MSP/X.509 identity) · [ADR-003](adr-003-privacy-model.md)
(PDC membership and private payloads) · [ADR-004](adr-004-scale-topology.md)
(hierarchical MSP) · [ADR-010](adr-010-capability-versioning-policy.md)
(downgrade-not-vote) · [#57](https://github.com/dbbvitor/GlassChain/issues/57)
(trust model) · [#58](https://github.com/dbbvitor/GlassChain/issues/58)
(revocation, independent)

## Context

Every `glasschain-node` that starts with `--org` creates an organization Root
CA and issues its own TLS identity from it — then drops the `Organization`.
`Node::set_cert_verifier` was therefore never called outside tests, so
`cert_verifier` was `None` at runtime and the #47 private-payload org gate
evaluated `verification_required == false`: the PDC path **failed open to the
self-asserted `Hello` org**.

Installing a verifier naively would break federation: each node's verifier
trusts exactly one org, so every cross-org peer would be rejected. Closing the
fail-open requires deciding **how a federation of independently self-issued
organizations establishes cross-org certificate trust**.

Considered options:

- **Federation trust store** — a node is configured with the set of peer-org
  Root CAs it accepts (file or directory of PEM anchors).
- **On-chain MSP registry** — org root CAs committed as canonical records;
  trust anchors derived from the chain. Self-bootstrapping, but creates a
  chicken-and-egg problem for joining nodes and couples identity
  bootstrapping to ADR-010 capability gating.
- **Shared/hierarchical root** — a federation root CA issuing org
  intermediates (ADR-004's ladder note). Requires an out-of-band ceremony and
  a real PKI owner, neither of which exists.
- **Keep TOFU-only, make the fail-open explicit** — leaves a security control
  that reads as active inert in production.

## Decision

### 1. Cross-org trust is an explicit, operator-configured federation trust store

`CertChainVerifier` holds one **own-organization** anchor (the Root CA that
issued this node's identity) plus a set of **federation anchors** loaded from
configuration. A peer certificate is accepted if it chains to any anchor. With
no federation anchors the model is exactly the previous single-org model.

`glasschain-node` gains `--trust-store <PATH>` (requires `--org`): a PEM file
or a directory of `*.pem` files, each holding one or more `CERTIFICATE` blocks
— a bundle file works as-is. Loading errors are fatal (`exit(1)`): an operator
who asked for verification must not silently get fail-open.

Starting with `--org` but without `--trust-store` keeps verification off, and
the startup log says so explicitly. The fail-open is now an operator-visible
decision, not a silent default.

### 2. Unverified organizations are downgraded, not disconnected

A peer whose org certificate does not verify against any configured anchor
stays connected: it may sync the public chain and verify public history
(mirroring ADR-010's downgrade-not-vote pattern and the participation model's
rule that roles are separate axes). Every org-gated path — private-payload
send and receive (the #47 gate) — fails closed against the peer's
self-asserted org. This replaces the earlier disconnect-on-unverified-Hello
behaviour.

### 3. Trust anchors persist; TOFU fingerprints remain per-run

The trust store is a configuration file, so anchor trust survives restarts —
retiring the "no trust persistence" limitation for *anchors*. The in-memory
TOFU fingerprint registry keeps its per-run lifetime; the address-bound,
in-memory TOFU limitations documented in the README remain in force.

### 4. Revocation stays independent (#58)

The trust store answers "which organization roots do we trust"; #58 answers
"has this member certificate been revoked since." A CRL/OCSP check slot is a
natural extension inside `CertChainVerifier` once revocation semantics are
decided; nothing in this ADR precludes or requires it.

## Consequences

- The PDC org gate is now real in production whenever `--org` and
  `--trust-store` are both supplied; the `Hello` org alone can no longer
  receive private payloads from a verifying node.
- The verifier is multi-anchor; `from_org`/`from_pem`/`from_der` semantics are
  unchanged, federation anchors are additive.
- Trust-store distribution between operators is manual and out-of-band. That
  is accepted: federation membership changes at governance pace, and an
  on-chain registry (the self-bootstrapping option) remains available later
  behind the same seam if the federation outgrows file distribution.
- `AGENTS.md`'s security note is updated: TOFU remains the default for
  fingerprint pinning, but local-CA verification of peer *organizations* is no
  longer forbidden — it is opt-in via `--trust-store`.

## Validation

- `glasschain-identity`: unit tests for federation-anchor acceptance,
  own-org coexistence, untrusted-org rejection, and multi-root bundle loading
  (`cert_verifier.rs` §15).
- `glasschain-network`: `pdc_distribution::federation_trust_store_enables_cross_org_payload_delivery`
  — a cross-org payload is delivered to a member holding the writer org's
  anchor and withheld from a member holding only its own anchor; the solo
  member also proves an unverified peer stays connected.
- `protocol_security::hello_with_unverified_org_certificate_stays_connected_but_unverified`
  — pins the downgrade-not-disconnect contract.
