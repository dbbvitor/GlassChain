# Plan — Zero-trust deployment and verification gaps

**Status:** active; proposals below are not implemented or an audit certification
**Reviewed:** 2026-09-05 against `f7b434e`
**Related:** [ADRs](../../docs/README.md), [performance](performance.md),
[post-quantum](post-quantum.md), [source-comment debt](deferred-code-debt.md).

## Goal

Make untrusted input safe across every admission, recovery and authorization
path while preserving privacy and deterministic history. Mechanisms such as TLS,
CRLs and endorsement exist, but their presence and closed tickets do **not**
establish complete zero-trust enforcement. The previous assertion that all
mechanisms were “built and correct” was too broad and is withdrawn.

## 1. Verified baseline

| Control | Current state |
|---|---|
| Peer TLS and TOFU | Default transport; address-bound in-memory fingerprints. Does not alone prove claimed organization membership. |
| Federation chain verification | Node startup installs a verifier with both `--org` and `--trust-store`; otherwise org claims remain unverified. |
| CRLs/intermediate CAs | Verification rejects missing/stale/revoked status when the verifier is configured; startup-loaded files are not automatic refresh or reauthorization of established sessions. |
| Endorsement provider | Attached under `--org`, initially registering local identity; enforcement also depends on active capability. Not complete remote certificate-derived principal management. |
| Governance defaults | ADR-012 uses endorsement carriers for capability activations/state commitments. Advisory record signatures do not become independently verified credentials. |
| BFT votes/certificates | Default-off staged implementation with verification code; context binding, receipt lifetime and historical verification need the checks in §8. |
| PQ transport | Both providers may be compiled; runtime selection and negotiated group need path-specific tests, not a conclusion from `features = ["ring"]` alone. |

## 2. ZT-1 — Fail closed on unverified organizations

[Org-gated fail-open default](https://github.com/dbbvitor/GlassChain/issues/86)
tracks `node.rs`'s PDC gate:
`!verification_required || sender_verified == Some(true)`, where
`verification_required = s.cert_verifier.is_some()`. A startup warning does not
prevent a no-verifier deployment trusting an asserted organization.

Plan: require verified membership on private send, receive and reconciliation
paths. Preserve public-history observation for downgraded peers under ADR-011.
Audit *all* call sites and configuration combinations; this is not just inverting
one boolean. Bind the presented organizational credential to the peer/key/session
with proof of possession, not merely possession of a public certificate string.

The earlier issue proposes `--insecure-unverified-orgs`; this is **not an accepted
or shipped flag**. First demonstrate that public-only local workflows still work
and use real test credentials for PDC demos. Any development exception needs an
explicit bounded threat model and decision; do not introduce another production
bypass or env-var kill switch to make tests pass.

Acceptance: absent/invalid verifier and forged org fail closed for private data;
public sync remains available only under its own validation rules; a copied
certificate without its private key cannot impersonate an organization.

## 3. ZT-2 — Aggregate rejection versus attributable evidence

Ed25519 batch verification with sequential attribution was superseded by BLS.
A failed aggregate is not sufficient to blame a named bitmap signer: a sender
could simply have fabricated the aggregate. Reject it; do not penalize its
claimed signers without independently authenticated evidence.

ADR-014 and the consensus docs now distinguish aggregate rejection from
vote-level evidence. **That distinction is not proof the current evidence path
is complete.** `BftVote::vote_message` authenticates a block hash but not the
routing height/round/phase, and `handle_vote` rebuilds receipts per call (§8).
Context-authenticated evidence and end-to-end detection tests must precede any
governance-driven exclusion. A cache of all individual signatures inside each QC
is not required; a bounded live receipt journal is a different concern.

## 4. ZT-3 — Transport PQ readiness

[post-quantum.md](post-quantum.md) owns provider choice, negotiation tests and
long-term evidence planning. Inventory runtime paths first; a compiled provider
is not proof it is selected. Preserve TLS signature verification, certificate
fingerprint binding and TOFU when configuring `X25519MLKEM768`.

Hybrid key exchange is the priority for data with long confidentiality lifetimes.
Optional PQ archival evidence addresses authenticity over time, not encryption,
live validator security or legal qualification. A shared dependency review must
not delay unrelated confidentiality improvements.

## 5. ZT-4/5 — Principal and key lifecycle

- [Certificate-bound MSP principals](https://github.com/dbbvitor/GlassChain/issues/87)
  owns source debt D4. Derive principal and signing key from verified credentials
  plus proof of possession; support remote principals. Specify revocation,
  expiry and deterministic historical authorization before wiring evaluation.
- [Persist the TOFU registry](https://github.com/dbbvitor/GlassChain/issues/88)
  remains a separate accepted-limitations decision. Durable pins need legitimate
  key rotation, corruption handling, recovery and an operator audit trail—not
  simply a file. Persisted node identity/key custody is related but not identical.
- Define trust-store/CRL refresh and established-session reauthorization. A check
  made during Hello cannot promise indefinite membership after expiry/revocation.
  Keep external retrieval off deterministic commit/replay paths and use explicit
  historical evidence for already-committed authorization.

## 6. Cryptographic backend review

[Backend decision](https://github.com/dbbvitor/GlassChain/issues/85) groups the
`aws-lc-rs` transport and `blst` consensus investigations. It should evaluate
specific providers, not invent a blanket ban on C: `ring`, `aws-lc-sys` and other
native dependencies already exist in the graph; the Rust unsafe lint scopes our
workspace code, not all dependencies.

ADR-014's measured-need condition for revisiting the pairing backend is met by
the failing 300-validator run. **Neither a 10× gain nor a transparent feature-only
swap is established.** Verify available APIs, domain separation, PoP/subgroup and
identity checks, key serialization and platform support. Compare an aggregate-
public-key verification path as well as backend changes; a smaller safe algorithm
may remove more work than replacing each pairing with a faster one.

Transport and consensus are separable migrations with different threat models.
Review supply-chain/audit evidence, license, CPU portability, performance and
Ubuntu/macOS/Windows builds per dependency. Do not interpret this plan as
approval of a new primitive, backend, certificate profile or algorithm lifetime.

## 7. Source-comment debt and compliance gates

All six ponytail markers and the one TODO have dispositions and acceptance tests
in [deferred-code-debt.md](deferred-code-debt.md). Zero-trust priorities:

- **D1 governance bootstrap:** keep fail-closed defaults; provision principals
  and verify scoped policy changes without weakening fixed operation defaults.
- **D2 recall:** resolve whether unilateral regulator action or independent
  organizations are required, then bind that policy to record scope. A learning
  model cannot supply the missing legal authority.
- **D4 certificate-bound principal registration:** the lifecycle work in §5.
- **D5 retention:** recover expiry discovery and schedule physical deletion after
  restart. Denying expired reads is not erasure; account for replicas/backups.
- **D6 triage:** restart-safe discovery before unattended recalls; recovery must
  not duplicate side effects. Coordinate a small storage scan with D5 only if
  both require it.

LGPD/ICP/ANVISA claims require applicable profiles, legal basis, access policy,
retention and evidence review. Public identifiers or digests can remain linkable;
“no certificates on-chain” is not a blanket privacy finding. Optional archive
preservation/legal holds and private-payload purge need distinct approved scopes.
No new ZK identity stack, FL dependency or public IPFS storage is justified by
these gaps.

## 8. Safety prerequisites before fast paths or performance claims

These are additional code observations, **not extra TODO markers** and not fixes
made by this documentation change:

1. **Authenticate consensus context.** `core/src/bft.rs::BftVote::vote_message`
   signs only a domain plus hash; `sign`/`verify` treat height/round/phase as
   routing metadata, despite a conflicting verification doc comment.
   Specify chain/epoch/height/round/phase binding for votes and QCs; test tampering
   each field, cross-phase replay and cross-network reuse. Account for wire and
   history compatibility before changing signed bytes. Do not claim a bare pair
   of hash signatures proves same-round/phase equivocation.
2. **Exercise detection through the network.** `node.rs::handle_vote` creates
   `VoteReceipts::default()` for each vote and seeds only from already detected
   proofs, not ordinary prior votes. Plan a bounded per-context receipt lifetime
   and a two-conflicting-vote integration test; valid votes across different
   rounds/heights must not falsely implicate a signer. Unit tests of a standalone
   `VoteReceipts` instance do not establish this wiring.
3. **Verify history at every entry.** `Message::Chain` enforces endorsements then
   calls `Ledger::try_replace_chain`; the default core history checker validates
   structure/capabilities rather than full BLS verification under every historical
   validator set. Audit sync and restart in addition to live `Message::Block`.
   Reject structurally plausible bad QCs, inactive algorithms and invalid registry
   transitions at each path before claiming verified history.
4. **Bound verification work.** Round delivery uses an unbounded vote channel;
   cryptographic verification runs inside state-lock sections. The round driver
   also resets `timeout(phase_timeout, recv())` for each message and collects
   votes before deduplication at aggregation. Test continuous stale/duplicate
   traffic against an absolute phase deadline and count only distinct eligible
   voters toward quorum. Profile overload/replay and use bounded queues/context
   filtering without trusting unverified votes or losing safety-critical state.

These take precedence over speculative rollback features. Define and test the
current driver's safety/liveness first; backend speed cannot repair missing
context binding or verification gates. No automatic exclusion or production
safety claim follows from the current local benchmark.

## Validation and next steps

Plan the §8 regressions first, then D1/D2/D4/D5/D6 and the independently testable
TLS negotiation work. Use current APIs/test harnesses; preserve default/all-feature
behaviour until an explicit adoption decision. Every implementation runs the
workspace gates and adds its named failure-case test. WAN/resource scenarios
and honest metric labels are specified in the performance and browser demo plans.
The proposed web bridge adds Host/Origin/session, command authorization and
server-side PDC filtering requirements; it is not implemented. Keep validator
keys off the browser and any WebGPU buffers.

Existing issues retain their identity, but earlier descriptions such as “one
inverted boolean,” “C is a new universal blocker,” or “attribution is intact”
need this qualification when those tickets are refined. No security relaxation
or architecture decision is authorized by closing a planning task.
