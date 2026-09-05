# GlassChain documentation

In-depth technical documentation. [`../README.md`](../README.md) is the overview
and quick start; everything here goes deeper.

Every document is written **against the shipped code**, not against the plan.
Where something is designed but not yet wired, the documents say so explicitly
rather than describing the intended end state — this repo has a documented
history of that failure mode, and correcting it is worth more than a tidy story.

## Start here

| Document | Read it when |
|---|---|
| [`architecture.md`](architecture.md) | You are new to the codebase. The crate map, the dependency rule, the provider seams, and a transaction traced end to end from submission to commit. |
| [`data-model.md`](data-model.md) | You need to know exactly what GlassChain stores. Transaction kinds, all 13 canonical schema v1 record families, blocks, write sets, capabilities. |
| [`consensus.md`](consensus.md) | You are evaluating or extending consensus. What runs today (Proof-of-Work), what is staged and default-off (BFT), the adoption gates, the membership ladder, and the performance target, baseline, and ordered path (§10). |
| [`privacy-and-identity.md`](privacy-and-identity.md) | You are reviewing security, or working on identity. MSP, certificate verification, endorsement policy, private data collections, and an honest inventory of what is inert at runtime. |
| [`workflows-and-contracts.md`](workflows-and-contracts.md) | You are building business logic. The contract/workflow split, the WASM host ABI, the flow framework, and watcher automation. |
| [`operations.md`](operations.md) | You want to run it. Build, flags, the REPL, the gRPC surface, storage, the wire protocol, and the security warnings an operator needs first. |
| [`liveness.md`](liveness.md) | You are planning a validator set. Failure-domain placement, jurisdiction floors, uptime and participation targets; these are operational guidance, not measured fleet guarantees. |

## Decisions

[`adr/`](adr/) holds the fourteen accepted architecture decision records. Read
the one covering your area **before** designing a change — several of them close
off options that look attractive from a blank page.

| ADR | Decision |
|---|---|
| [001](adr/adr-001-execution-layer.md) | WASM/Wasmtime is the only runtime; EVM compatibility is a deferred adapter seam, Solidity is out of scope |
| [002](adr/adr-002-consensus-finality.md) | Tendermint/CometBFT-class BFT; immediate deterministic finality; Proof-of-Work and finality-gadget designs rejected |
| [003](adr/adr-003-privacy-model.md) | Fabric-style private data collections: public commitments, private payloads point-to-point |
| [004](adr/adr-004-scale-topology.md) | One globally ordered chain, no execution sharding, off-chain state commitments, light-client ladder beyond the validator ceiling |
| [005](adr/adr-005-certification-and-audit.md) | Certification and audit are first-class signed, append-only processes over immutable lot commitments |
| [006](adr/adr-006-canonical-schema-v1.md) | 13 strict record families, registered extension namespaces, capability-controlled activation |
| [007](adr/adr-007-vm-state-semantics.md) | Explicit persistent writes, committed write sets, scoped public/PDC visibility |
| [008](adr/adr-008-endorsement-policy-model.md) | Fabric-style signature policies over verified MSP principals, distinct signer counting |
| [009](adr/adr-009-validator-eligibility.md) | One org, one vote; an objective published eligibility bar; epoch-or-height duty-roster rotation. Closes ADR-002's open questions 4–5 |
| [010](adr/adr-010-capability-versioning-policy.md) | Network-wide committed capability set gates every consensus-visible behaviour; height-based historical validation |
| [011](adr/adr-011-federation-trust-store.md) | Cross-org trust is an operator-configured federation trust store (`--trust-store`); an unverified org is downgraded, not disconnected |
| [012](adr/adr-012-signature-binding.md) | Capability activations and `state_commitment` records carry fail-closed governance defaults enforced through the endorsement layer |
| [013](adr/adr-013-certificate-revocation.md) | Revocation is fail-closed CRLs in the trust store, plus intermediate CAs; go-forward only, committed history stays valid |
| [014](adr/adr-014-bls-aggregated-certificates.md) | One BLS12-381 aggregate signature plus a signer bitmap; compact certificates with proof-of-possession registration |

## Evidence

- [`benchmarks/consensus-capacity.md`](benchmarks/consensus-capacity.md) — recorded
  PoW propagation and staged BFT finality runs. Local synthetic evidence is not
  a completed WAN testnet or production adoption gate.
- [Plans](../.agents/plans/README.md) — current roadmap, the complete source-comment
  debt inventory, and research proposals clearly separated from shipped features.

## For agents

- [`agents/`](agents/) — issue-tracker conventions, triage labels, and how the
  engineering skills should consume this repo's domain docs.
- [`../AGENTS.md`](../AGENTS.md) — the canonical project rules and invariants.
- [`../.agents/`](../.agents/) — working artifacts: the session handoff, the live
  programme plan, and durable findings. Not shipped documentation; expect it to
  be rougher and more provisional than anything in this folder.
