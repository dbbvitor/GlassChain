# ADR-004 — Scale topology: single chain with off-chain state commitments

**Status:** Accepted
**Date:** 2026-08-20
**Relates to:** §1.1, §3.2, §6.1, §8.2 · [ADR-002](adr-002-consensus-finality.md)
(validator ladder), [ADR-003](adr-003-privacy-model.md) (PDC hash commitments) ·
[ADR-007](adr-007-vm-state-semantics.md) (persistent VM state) ·
wayfinder [#24](https://github.com/dbbvitor/GlassChain/issues/24)

## Context

Two national-scale analyses set the design horizon at ~70M entities (all active
Brazilian corporate entities, plus MEIs, smallholders, and individual
logisticians) and asked whether the ledger survives that scale. Their shared
prescription — partition into channels/shards, replace the MSP with DIDs, batch
via ZK rollups — assumed raw events land on-chain (70M × 2–3 updates/day ≈
2,000–3,000 TPS, the failure mode they then design around).

Two things were already settled that change the answer: ADR-003's Option A
pattern (payloads off-chain, hash commitments on-chain), and ADR-002's note
that bounding the validator set is configuration, not redesign.

## Decision

1. **One globally ordered core chain.** No execution sharding. The core carries
   state commitments (Merkle root + counterparty MSP signatures), high-level
   custody handoffs, NF-e hashes, certification/audit anchors, and the explicitly
   persistent public contract-state write sets or PDC write-set commitments
   governed by [ADR-007](adr-007-vm-state-semantics.md) — never raw line-item
   telemetry or private evidence.
2. **Raw events live off-chain** in the edge / private-data layer (ADR-003 PDCs
   and authorized counterparty infrastructure). The commitment makes them
   tamper-evident and globally ordered without putting them on-chain.
3. **Membership ladder.** Every member is MSP-identified. Members who operate
   validators vote; the rest are authenticated light clients. Full
   participation while the validator count is within the practical BFT ceiling
   (~300); beyond it, a bounded institutional validator set. The consensus
   family (Tendermint-class, ADR-002) is unchanged at every rung. Persistent VM
   state follows ADR-007: only explicit, scoped writes enter the committed record;
   high-frequency edge state remains an off-chain commitment.
4. **Hierarchical MSP.** Consortium root CA in v1; intermediate CAs (banks,
   cooperatives, ERP vendors) delegate issuance. ICP-Brasil e-CNPJ/e-CPF
   certificates are verified as *onboarding credentials* — not the ledger's
   root of trust. (Flagged: if literal ICP-Brasil root anchoring was intended,
   revisit.)
5. **Throughput arithmetic.** On-chain load is commitments, not events. ~2k
   events/s at 70M entities becomes tens of commitments/s after aggregation.
   The core chain's ceiling is not the binding constraint; the off-chain event
   layer's is. Persistent VM state is intentionally narrower than raw event
   volume and must not be used to smuggle telemetry onto the global chain.
6. **Consensus input guardrail.** The BFT core receives approved public canonical records and commitment envelopes. Private quantities, pricing, counterparties, raw evidence, and unbatched high-frequency telemetry remain outside the globally replicated pending pool. Public custody facts and other approved canonical records are not prohibited merely because they are not commitments.
7. **Capacity evidence.** The implementation must benchmark the compact GlassChain workload at 200 and 300 validators, including block latency/size, vote traffic, pending-pool backpressure, private-data dissemination separately, and WAN delay/partition recovery. No public 200–300-validator saturation number or ZK-only benchmark substitutes for this test.

## Consequences

- The transaction model gains a **state-commitment kind** (Merkle root + MSP
  multi-signatures). Canonical schema v1 (wayfinder #19) and the Stage 3
  workflow engine must treat batch anchors as first-class.
- Certification and audit records become first-class signed anchors that
  reference those immutable lot commitments. Evidence manifests and issuer,
  scope, validity, and status fields are globally ordered; raw evidence remains
  in the private/off-chain layer per ADR-005.
- "Where do VM state mutations land?" (wayfinder [#21](https://github.com/dbbvitor/GlassChain/issues/21))
  is resolved by [ADR-007](adr-007-vm-state-semantics.md): explicit persistent
  writes are represented in the committed block, replay consumes those write
  sets, and ephemeral output/high-frequency flow state stays off-chain.
- The indexer consumes commitments and certification/audit anchors; analytics
  over raw events or evidence requires authorized off-chain payload retrieval
  (Stage 6 design note).
- The SEFAZ/NF-e adapter (Stage 5) is the canonical edge-ingress example:
  polled DF-e distribution → signed attestations → commitments. NF-e event
  semantics (emission, transit, delivery, cancellation) feed canonical schema
  v1 (#19).
- Fair timestamps come from CometBFT BFT Time. Full fair ordering is NOT
  claimed: a proposer orders within its block. Accepted because the validator
  set is bounded, institutional, rotating, and endorsement-gated.

## Rejected alternatives — do not re-propose without reopening this ADR

- **Channel/sub-ledger sharding** — solves a load the decoupled model doesn't
  have; conflicts with the single-global-chain rationale ADR-003 relies on;
  cross-shard custody is the classic hard problem. Channels remain the privacy
  boundary (ADR-003), a different axis. **This resurfaces under rebranded names
  — "regional channels", "local-first consensus", "sub-clustered voting",
  "cross-shard rollup finality". All are the same rejected proposal.** Three
  concrete breakages to cite when it returns: (a) a recall must be traversable by
  parties who were never counterparties (ADR-003), and product crosses regions, so
  sharding puts the classic hard problem in the safety-critical path; (b)
  `ProvenanceIndex`, `MetadataTrustScore`, SNCM validation, and the flattener all
  assume a globally ordered chain they can index; (c) supply chains are inherently
  cross-region — farm → co-op → port → exporter — so nearly every transaction
  worth ordering is a cross-shard transfer and the sharding buys almost nothing.
  Decision 1 above and ADR-002 decision 4 ("local aggregation is pure transaction
  ingress; its confirmations are never treated as committed state") both forbid it.
- **DID/VC identity stack** — the federation requirement is met by X.509
  intermediates (webpki already verifies chains); Brazil has a legal PKI
  (ICP-Brasil); replacing MSP/cert_verifier/gRPC-auth buys nothing. VCs may
  appear later as an edge attestation format.
- **ZK rollups** — deferred until batch contents must be proven without
  revelation. Merkle root + signatures suffice for tamper-evidence.
- **SCITT as a replacement transparency log** — not required for v1; a future
  edge-format adapter must not replace the global chain without reopening this ADR.
- **ZK validiums and data-availability committees** — introduce new proof,
  custody, and availability dependencies without a v1 requirement.
- **KERI/ACDC or DID/VC identity replacement** — MSP/X.509 federation remains the
  identity model; future credential formats belong at an ingress adapter.
- **IPFS / decentralized object storage** — off-chain payloads live in the
  PDC/counterparty layer; no new storage-network dependency.
- **"aBFT" settlement core** — per ADR-002: no Rust production path, no 200+
  validator evidence, and nothing here needs asynchrony.

## Open questions

1. **The aggregation ratio is unsized.** Decision 5's arithmetic ("~2k events/s
   becomes tens of commitments/s") depends entirely on how many raw events one
   state commitment covers, and that number has never been chosen or measured. It
   is a straight multiplier on core-chain load — 100 events per commitment versus
   10,000 is a 100× difference — and it is the **highest-leverage scaling knob in
   the whole design**, well ahead of anything in the consensus layer. Blocked on
   the state-commitment record kind, which is specified by
   [ADR-006](adr-006-canonical-schema-v1.md) but not implemented
   (`requirements-alignment.md` §4.1). Size it empirically against the SEFAZ/NF-e
   ingress path once that lands.
2. **The off-chain event layer's ceiling is unmeasured.** Decision 5 asserts it,
   not the core chain, is the binding constraint — but no measurement exists.
   Scaling effort belongs on the dissemination path (PDC distribution,
   reconciliation, `LibP2pNode`) and the read path (indexer → RPC), neither of
   which creates any membership distinction. See
   [`participation-model.md`](../memories/participation-model.md) §7 for the
   three-axis decomposition.
