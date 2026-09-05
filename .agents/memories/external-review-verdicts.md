# External review verdicts — architecture report, 2026-09-02

**Learned:** 2026-09-02
**Source:** an external review report ("Distributed Inventory System" notebook)
assessed against the working tree at `fed76c7` and against primary sources.

**Current reading:** entries below are dated assessments, not current-state
specifications. The final 2026-09-05 HotStuff/PQC/learning-loop entry supersedes
blanket earlier conclusions about compliance, source usefulness and scaling.

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

---

## 2026-09-03 — "Zero-Trust Hybrid BFT" report (VRF sortition, SGX attestation, ephemeral mTLS, slashing)

Reviewed against ADR-002, ADR-011, and the participation model. **Verdict: ~10%
adopt, the rest already rejected by name or inapplicable.** The report targets a
public/permissionless threat model; GlassChain is a permissioned consortium of
known, mutually-distrusting rival orgs.

### Adopted

- **Equivocation proofs → governance-driven validator exclusion** — filed as
  [#75-adjacent ticket](https://github.com/dbbvitor/GlassChain/issues/76) (the
  self-verifying two-conflicting-signatures record; the governance *act* of
  exclusion belongs to ADR-002 open questions 4/5 churn mechanics). Cheap,
  reuses ADR-008 verification machinery.

### Already true — the report's "Standard BFT" column misdescribes us

- Every vote individually signed and cryptographically verified
  (`consensus.rs::verify_certificate`); no aggregate accepted on trust.
- No perimeter trust: TLS default, TOFU pinning, fail-closed CRLs (ADR-013),
  downgrade-not-disconnect (ADR-011), fail-closed governance defaults
  (ADR-012).
- Explicit timeouts and QC-proof view changes are the standard Tendermint round
  protocol, not missing work.
- BLS threshold signatures = performance Step 4 (already sequenced, with the
  §3 correction that aggregation compresses the *certificate*, not the O(n²)
  vote gossip). HotStuff pipelining = Step 5. Light-client finality = ADR-004
  ladder. Nothing new.

### Rejected (with reasons, per the established pattern)

- **VRF sortition / rotating K-of-N committee** — rejected by name in ADR-002.
  Governance standing attaches to membership, not to winning a lottery;
  rotation is the first step toward the tiers/cartel failure the participation
  model documents three design passes falling into. Our scalability claim is
  deliberately *all n participating with deterministic ⅔ finality* — committee
  sampling is how other systems weakened that constraint.
- **Weighted validators `authority × uptime × reputation`** — voting power is
  ADR-002 open question 4, deliberately deferred; a node *reputation* score
  would collide with `MetadataTrustScore` (lot provenance completeness, not
  behavior — the documented fact-check); there is no stake/fee model anywhere
  (§2.4) to weight.
- **Stake slashing** — no balances exist. Ejection on equivocation is a
  governance act (the adopted ticket), not cryptoeconomics.
- **Hardware remote attestation (SGX/TDX/SEV) as an admission gate** — swaps
  the root of trust from federation governance to Intel/AMD, excludes member
  infrastructure that can't run confidential computing (and every smallholder
  in ADR-004's horizon), makes binary attestation brittle across rebuilds, and
  nothing in the 26 requirements asks for it.
- **Ephemeral keys / SPIFFE-style rotating identities** — contradicts ADR-008's
  stable key→principal directory (the security boundary) and ADR-011 TOFU
  pinning; misattribution under a compromised ephemeral key is *worse* than the
  channel hygiene it buys. Identity-backed mTLS already exists.

---

## 2026-09-05 — third "Distributed Inventory System" notebook report

Assessed against `main` @ `f7b434e`. **Verdict: it is the 2026-09-02 report
again**, re-generated from the same notebook, with the same headline priority —
which was already stale when first assessed and is now stale twice over.

| Report claim | Verdict |
|---|---|
| "Main priority: completing the end-to-end integration of the PDC distribution layer" | **Stale, second time.** Shipped in #46/#47 *before* the first report was written. Ruled stale on 2026-09-02; unchanged since |
| "...alongside the zero-trust identity verification pipelines" | **Stale.** #57/#58/#59 all closed — verifier wired at startup (ADR-011), fail-closed CRLs (ADR-013), MSP provider attached under `--org` |
| Narwhal/Bullshark DAG separation of dissemination from ordering | **Unchanged verdict:** correct but orthogonal. Mempool layer behind `ConsensusProvider`, performance Step 6, gated on a measured trigger |
| "Transition towards leaderless or optimistic fast-path consensus" | **Unchanged verdict:** in-family latency optimization (Step 5), not a family change. ADR-002 stands |
| `zk-X509` for on-chain certificate-chain verification | **Unchanged verdict:** real work, inapplicable. Solves a public-chain metadata leak we do not have |
| Off-chain PII / on-chain attestation separation, PDCs committing only hashes | **Unchanged verdict:** satisfied by design (ADR-003). Nothing certificate-shaped is ever committed |
| Off-chain analytics isolated from the consensus runtime | **Unchanged verdict:** correct, and shipped (#39) |

### Implication — the useful finding is about the *source*, not the content

This report contains **zero new information** relative to the 2026-09-02
assessment, and its stated "main priority" has been shipped for over twenty
commits. The notebook is regenerating from a corpus that does not include our
repository state, so it will keep proposing work that is done.

**Treat further reports from this source as a literature check, not a status
report.** Its value is confirming that our ADRs match the published state of the
art — which it does, consistently. Its priorities are worthless without first
checking closed tickets, and the standing rule from 2026-09-02 stands unchanged:
**when an external report names a priority, check the closed tickets first.**

### What this review *did* find — by reading our own code, not the report

Both findings came from auditing the repo against its own plans, and neither
appears in any external report:

1. **Performance Step 2 shipped and was deleted three commits later** by
   ADR-014, taking a documented zero-trust property (per-signer attribution on
   certificate verification) with it. Nothing recorded the trade. Now ZT-2 in
   [`../plans/zero-trust.md`](../plans/zero-trust.md) §3.
2. **Two plans are blocked on the same unasked question.** Post-quantum action 1
   (`aws-lc-rs`) and the 300-validator finality gate (`blst`) both need a C
   cryptographic backend, and ADR-014's own revisit condition for `blst`
   ("revisit only with a measured need") is now met by the measured 300-validator
   failure. Neither plan saw it because each treated the C toolchain as a local
   cost. Now [`../plans/zero-trust.md`](../plans/zero-trust.md) §6.

The pattern worth carrying: reconcile plans against each other periodically,
not just against the code. The prior assessment did not inspect the notebook's
private corpus; assertions about why its priorities were stale were inference,
not verified knowledge of that source.

---

## 2026-09-05 — HotStuff / PQ archival evidence / learning-loop report

**Scope:** the new report supplied in chat; no access to its underlying notebook
or unspecified DLT-LFL/BC-FL papers is assumed. It contains useful new proposals
as well as incorrect statements about the implementation. Assess them separately
rather than accepting its two-option follow-up framing.

| Claim or proposal | Verdict and resulting plan |
|---|---|
| GlassChain uses HotStuff-style consensus with speculative execution | **Incorrect as shipped.** `network/src/rounds.rs` describes Tendermint-shaped proposal/prevote/precommit and locking; no client-visible speculative execution API. Corrected `docs/consensus.md`; retain fast-path research in performance Step 5, not a speculative test suite for nonexistent code. |
| HotStuff-1 one-phase confirmation cuts latency | **Relevant research, not a drop-in optimization.** Its abstract describes early speculative client confirmations and a prefix-speculation dilemma. Distinguish tentative replies from finality; any design must prove fault/fallback behaviour and prevent pre-finality business side effects. |
| WAN overlays/madsim before cloud deployment | **Adopt benchmark work.** `tcp_partition.rs` already tears down established TLS sessions over real TCP; delay/jitter/bandwidth matrices are new work. Extend that proxy first. The one source TODO remains optional madsim-tokio migration, not absent fault testing. |
| DAG or Snow-family alternatives for contention | **Split the ideas.** Narwhal-like dissemination is a measured Step 6 candidate; it cannot eliminate application hot-key contention. DAG ordering and probabilistic Snow consensus are outside ADR-002, not an unreviewed overlay to add. |
| zk-X509 needed for private enterprise identity | **Not justified for this deployment.** We do not commit certificate chains to the public ledger. Fix certificate/principal binding, transport possession and lifecycle controls first; ZK proofs would not remove issuer trust, revocation freshness or endpoint authorization requirements. |
| PQ-signed Merkle roots over legacy signatures | **Add scoped archival-evidence research, not automatic adoption.** RFC 4998 already defines batched hash-tree timestamps and renewal. Bind original signed bytes, signature/algorithm context, validation evidence and inclusion proofs; choose trusted time and renewal policy. A signature over a claimed time is not itself trusted timestamping. |
| Current endorsements are ECDSA/RSA | **Incorrect for native carriers.** MSP endorsements use Ed25519; staged consensus uses BLS. Imported enterprise PKI formats are a separate interoperability boundary. An archive must preserve actual bytes/algorithms rather than relabel them. |
| DLT-LFL mandates federated learning | **Unsupported.** No primary citation or applicable regulatory mandate supplied. Map Sense/Decide/Adapt to existing events/rules/flows; add offline outcome evaluation for Learn. §6.5 remains a deferred SHOULD with task, dataset, benefit and privacy gates. |
| Keep model parameters off-chain, optionally IPFS | **Keep the off-chain boundary, not a dependency mandate.** Gradients and model parameters can disclose data; hashes may be linkable. Prefer authorized existing storage/export, and require access, retention and poisoning controls before any FL adapter. |
| GUI speculative latency and view-change metrics | **Partly adopt.** Show admission, verified finality, round changes/timeouts and recovery separately. Speculative latency is unavailable until such an interface exists; never label early admission as final. |
| High-frequency flattening memory benchmarks | **Adopt.** `AnalyticalFlattener.records` is a growing vector; bounded event-bus buffers do not bound projections. It ingests AssetRegistration only, and node commit processing invokes it. Measure retained rows/RSS, lag, query/replay costs and impact on finality with representative input. |

### Source-comment reconciliation

The previous response understated the debt by calling every marker clean.
**Six ponytail + one TODO** are now individually planned in
[deferred-code-debt.md](../plans/deferred-code-debt.md), with source locations,
triggers, next steps and acceptance checks. Two observations matter:

- `TransientStore::get` denies expired reads after restart, but the in-memory
  index cannot discover old keys for deletion. This is a retention gap, not a
  harmless cache detail (D5).
- `FlowTriage` names the already-closed purchase/recall tickets as its future
  trigger. Checkpoints survive, but unattended discovery still does not (D6).
  Source history references do not prove an operational recovery requirement met.

Also correct the earlier scaling assertion: a survey of validator counts cannot
prove that protocol optimization cannot surpass 300. It may improve the feasible
operating point; demonstrate 300 first, then test beyond it under the same trust
and quorum model. Fixed-fraction quorum delay is not universally proportional
to the maximum-of-n Pareto formula previously quoted.

### Additional safety observations while checking the report

Not source-comment markers and not runtime fixes in this pass:
`BftVote::vote_message` signs a hash but not round/phase metadata;
`handle_vote` recreates the receipt tracker per message; `Message::Chain`
reaches structural rather than full historical BLS validation. These contradict
the prior blanket claim that vote attribution and every verification path are
complete. The [zero-trust plan §8](../plans/zero-trust.md) records focused
regressions and the design questions before fast paths or production claims.

### Sources re-read for this report

- [HotStuff-1 v3](https://arxiv.org/abs/2408.04728v3), abstract read 2026-09-05:
  early speculative confirmations, prefix dilemma and view slotting; not proof
  that our driver implements it.
- [Narwhal and Tusk v4](https://arxiv.org/abs/2105.11827v4), abstract read
  2026-09-05: dissemination/ordering separation, WAN evaluation and latency
  costs under faults. Its numbers are not GlassChain capacity claims.
- [RFC 4998](https://www.rfc-editor.org/rfc/rfc4998.html), §§1.1–1.3, 4–5, 7,
  read 2026-09-05: archive evidence, Merkle batch timestamps, validation material,
  signature/hash-tree renewal **before** algorithms/evidence become unreliable.
- [NIST FIPS 204](https://csrc.nist.gov/pubs/fips/204/final) and
  [FIPS 205](https://csrc.nist.gov/pubs/fips/205/final), publication pages read
  2026-09-05: standardized ML-DSA and SLH-DSA, respectively. Algorithm standards
  do not establish ICP-Brasil credential acceptance or legal sufficiency of an
  archival service. Detailed qualifications live in the PQ plan/research memory.

No new architecture was approved by this report. Priority: source-debt/security
and actual-driver measurement first; archival-evidence design can proceed
independently; FL and speculative consensus implementation remain gated.

### Owner follow-up — browser demo replaces desktop (2026-09-05)

The owner changed the visual demo to a web app and asked to consider WebGPU.
The [demo plan](../plans/gui-demo-benchmark.md) and existing tracking issue now
supersede gpui rather than adding a second GUI. WebGPU is a rendering API,
with limited browser availability and secure-context/device requirements
([MDN](https://developer.mozilla.org/en-US/docs/Web/API/WebGPU_API), read
2026-09-05), not a consensus or frontend application framework. A same-origin
Rust demo bridge, accessible baseline and measured GPU fallback are planned;
none is shipped. This is an explicit product-direction decision, not an
architecture change inferred from the earlier literature report.
