# External review verdicts — architecture report, 2026-09-02

**Learned:** 2026-09-02
**Source:** an external review report ("Distributed Inventory System" notebook)
assessed against the working tree at `fed76c7` and against primary sources.

## Finding

The report's four architectural sections are **broadly aligned with our accepted
ADRs**, but it contains one stale priority, two claims that read as ADR-reopening
and are not, and one real-but-inapplicable technology. Two genuinely unplanned
gaps surfaced while checking it — neither of them the ones the report names.

## Verdicts

| Report claim | Verdict |
|---|---|
| "Main priority: completing end-to-end integration of the PDC distribution layer" | **Stale.** Shipped: #46 (`f91f0f2`, `Message::PrivatePayload`, wire `/3`, transient store, four-boundary enforcement) and #47 (`fa69e2d`, pull reconciliation via `RequestPrivatePayload`, wire `/4`, 72h retention/purge, cert-verified delivery). |
| "Transition towards a leaderless or optimistic fast-path consensus" | **Not our decision, and not implied by the research.** ADR-002 is Tendermint/CometBFT-class and stands. See below. |
| Separating dissemination from ordering (Narwhal/Bullshark DAG) | **Correct but orthogonal.** Narwhal is explicitly a *mempool* layer that composes with partial-sync BFT ordering (Narwhal-HotStuff). It is a throughput layer that could sit behind `ConsensusProvider`; it is not a consensus-family swap. Bullshark/Mysticeti *are* DAG-based ordering (a family change) — and no reusable standalone Rust crate exists for either: Sui's is monorepo-embedded (`consensus/core` is Sui-coupled), and `MystenLabs/narwhal` / `facebookresearch/narwhal` are archived. |
| Single-phase/speculative fast paths (HotStuff-1, SBFT) | **In-family latency optimizations**, ADR-preserving. HotStuff's own framework subsumes Tendermint. SBFT was geo-deployed at 209 replicas (f=64) at ~2× PBFT throughput — evidence *for* our validator ceiling, not against our family. |
| "Off-chain PII / on-chain attestation separation required" | **Already satisfied by design.** Certificate material lives in memory (`Organization` root CA, `Identity.certificate_pem`) and on the wire (`Message::Hello.certificate_pem`); the peer registry stores only fingerprint + org. **Nothing certificate-shaped is ever committed to a block** — blocks carry hash commitments only (ADR-003). |
| `zk-X509` for on-chain cert-chain verification | **Real work, inapplicable.** arXiv:2603.25190 (Tokamak Network) with an SP1-zkVM Rust prover and a Sepolia deployment — but single-author, unpeer-reviewed, testnet-only, and it solves a *public-chain* metadata-leak problem GlassChain does not have. |
| "ICP-Brasil X.509 under MP 2.200-2 required for legally binding signatures" | **Real law, cuts the other way.** MP 2.200-2 Art. 10 §2º expressly preserves other means of proving authorship and integrity — *including non-ICP-Brasil certificates* — "desde que admitido pelas partes". Lei 14.063/2020 applies to public-sector interactions (Art. 2º), so a private federated ledger needs no ICP-Brasil chain. **New pharma-relevant datum:** Lei 14.063 Art. 13 mandates a *qualified* signature for controlled-substance e-prescriptions, and Art. 4º §2º mandates revocation mechanisms. |
| LGPD constraint on immutable ledgers | **Real, and already avoided.** Art. 18 III/VI (correction/erasure) vs immutability is a genuine tension, resolved by Art. 16-I where retention serves a legal duty — and moot when no PII is written on-chain, which is our case. ANPD guidance could not be retrieved (site under a temporary election-period restriction). |

## The two gaps that actually surfaced

Neither is what the report flagged. Both confirmed by reading the code.

### 1. Certificate verification is inert in production — and it fails *open*

`glasschain-node/src/main.rs` builds an `Organization` (root CA in hand) to
issue the node's TLS identity, then drops it. `Node::set_cert_verifier` is
called **only from two integration tests**
(`pdc_distribution.rs:313`, `protocol_security.rs:835`). So at runtime
`NodeState.cert_verifier` is `None`, and the #47 private-payload org gate —

```rust
let verification_required = s.cert_verifier.is_some();   // node.rs:2859
let sender_ok = /* membership */ && (!verification_required || sender_verified == Some(true));
```

— evaluates `verification_required == false`, accepting the **self-asserted
`Hello` org**. The same is true of the `org_verified` check at `node.rs:2453`.

**This is not a missing one-liner.** Each node self-issues its own org root CA,
so `CertChainVerifier::from_org(&own_org)` would reject every cross-org peer —
the "no shared CA" limitation `AGENTS.md` records as accepted. Wiring it
requires a federation trust-store / CA-distribution decision first. **Do not
"fix" this by installing a single-org verifier in the node binary.**

Related: `msp_policy.rs`'s module-level `ponytail:` note ("the directory stands
in for certificate-bound MSP verification") is the same gap seen from the
identity side.

### 2. `record.signatures` are structurally count-only, permanently

`canonical.rs` (`state_commitment` counterparty signatures) and
`capability.rs` (`CapabilityActivation.signatures`) both carried `ponytail:`
comments promising cryptographic verification "lands with the endorsement engine
(#37/#45)". **#37 and #45 shipped and closed without touching them** — the
engine verifies `Transaction.endorsements` carriers, and `grep` confirms
`endorsement.rs` never reads `.signatures` at all. The comments were stale
forward references to closed tickets (the failure mode `debt-gap-handoff.md`
warns about: *"grep for a ticket number before closing it"*). Rewritten
2026-09-02 to state the real ceiling and that binding them needs its own
decision.

## Implication

- **Do not reopen ADR-002** on the strength of DAG-BFT or fast-path literature.
  If throughput ever demands it, the lazy path is a Narwhal-style dissemination
  layer *behind* `ConsensusProvider` — a mempool change, not a family change.
  The blocking facts are unchanged: Malachite is still alpha/unaudited (v0.5.0,
  last commit 2025-10-21, project moved to Circle), and no primary source
  publishes 180–300-validator figures for any of these protocols.
  **Followed up 2026-09-02** in [#62](https://github.com/dbbvitor/GlassChain/issues/62):
  asked directly whether fast paths or a DAG mempool could push past 300, and the
  answer is no — no production system runs deterministic per-round ⅔ finality
  with all `n` participating beyond ~209, and **no Narwhal-family paper claims
  validator-count scaling** (the claims are throughput and latency only). The
  useful finding was elsewhere: the quorum certificate is ~5× oversized purely
  because JSON renders `Vec<u8>` as decimal arrays, and certificates are not
  persisted yet, so the fix is nearly free today. See
  [`../plans/performance.md`](../plans/performance.md).
- **Do not add a ZK identity stack.** ADR-004/ADR-010 already rule out DID/VC,
  ZK rollups, and ZK validium; zk-X509 is on the far side of that line and
  addresses a problem our PDC design avoids.
- **Certificate revocation (CRL/OCSP) is the highest-value real identity gap**,
  and Lei 14.063 Art. 4º §2º is external pressure for it. It is genuinely
  unplanned — no ADR covers it.
- When an external report names a priority, **check the closed tickets first.**
  This report's headline priority had been shipped for two commits.
