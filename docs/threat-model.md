# Threat Model

GlassChain's security assurance case: what the system protects, what it
cannot yet protect against, and where each mitigation lives. Written per the
OpenSSF Best Practices Gold criteria (`threat_model`, `security_assurance_case`).

Status: **pre-release**. Everything here describes the system as built;
"staged" items are labeled honestly. See the
[capability status](../README.md#capability-status--real-today-vs-staged)
section of the README before trusting any claim.

## 1. Assets (what an attacker could gain or destroy)

| Asset | Consequence if compromised |
|---|---|
| **Ledger integrity** | Reorder, erase, or forge committed supply-chain transactions; break the SHA-256 hash chain or a quorum certificate |
| **Transaction confidentiality (PDC payloads)** | Read pricing, quantities, or raw evidence held in private data collections (ADR-003) |
| **Identity material** | A member organization's Root CA or node identity key; a forged MSP certificate or endorsement |
| **Consensus finality** | Double-commit, equivocate undetected, or stall block production |
| **Contract execution isolation** | Escape the WASM sandbox, exceed fuel, or poison contract state |
| **Peer trust state** | Impersonate a peer (TOFU pin hijack), replay a stale certificate or vote |

## 2. Trust boundaries (where untrusted input enters)

1. **P2P wire** — JSON messages (`glasschain_network::protocol::Message`)
   framed with a 4-byte length prefix from an arbitrary peer over TCP. Every
   variant is attacker-controlled: `Hello` (identity claims), `Transaction`,
   `Block`, `Proposal`, `Vote`, `Precommit`, `PrivatePayload`.
2. **gRPC API** — unauthenticated service surface (`glasschain-rpc`);
   transaction submission, queries, event streams. Admin methods gated by
   `AdminGate` (ADR-017): certificate-bound admin principals only, fail closed.
3. **Storage seam** — pluggable persistence (`RedbStorageProvider`); state is
   rebuilt by replaying the committed chain on restart.
4. **WASM contract execution** — untrusted contract bytecode inside wasmtime
   with fuel metering (`glasschain-vm`).
5. **Configuration / operator inputs** — trust store (PEM anchors, CRLs,
   intermediates), identity file, CLI flags.

## 3. Threats and mitigations per boundary

### 3.1 P2P wire (transport)

| Threat | Mitigation | Evidence |
|---|---|---|
| Passive eavesdropping on peer traffic | TLS 1.3 is the default transport; optional `X25519MLKEM768` hybrid post-quantum KEX (`pq-tls` feature, `aws-lc-rs`) | `crates/glasschain-network`, `docs/privacy-and-identity.md` §1.5 |
| Impersonation of a peer on first connect | Certificate fingerprint pinned in a **durable TOFU pin registry** (storage-backed, survives restart); a re-issued certificate must carry a **signed rotation** by the pinned key | ADR-018-era design; `peer.rs`; [CONTEXT.md](../CONTEXT.md) "Durable TOFU pin" |
| Copied-certificate impersonation (org claims) | **Session-bound possession proof**: ed25519 signature over the RFC 5705 TLS exporter of the *live* session (#110) — a stolen PEM proves nothing | [CONTEXT.md](../CONTEXT.md) "Session-bound possession proof" |
| Revoked or bogus org certificate | `CertChainVerifier` performs real `rustls-webpki` chain checks against own-org Root CA + federation anchors (ADR-011); **fail-closed CRL** (ADR-013) — missing/expired/revoked all reject; OCSP staple verified locally, `revoked` fails the session closed, absent staples never upgrade the CRL result (ADR-017) | `crates/glasschain-identity`, `docs/privacy-and-identity.md` §1.1–1.4 |
| TLS downgrade / insecure mode abuse | `GLASSCHAIN_INSECURE_TLS=1` is a documented local-debugging escape hatch only; adding new kill switches is forbidden | [AGENTS.md](../AGENTS.md) security invariants |
| Malicious/oversized frames | 16 MiB `MAX_MESSAGE_SIZE` frame cap; serde deserialization rejects unknown algorithm discriminants and unknown fields | `protocol.rs`; fuzz targets `fuzz-wire` (§5) |
| Trust-store poisoning | Anchors/CRLs/intermediates load only from the operator-supplied `--trust-store`; reload is operator-signalled (`reload-trust-store` REPL), never a timer | ADR-011, ADR-017 |

### 3.2 gRPC API

| Threat | Evidence |
|---|---|
| Unauthenticated admin actions | `AdminGate` (ADR-017) requires certificate-bound `OU=admin` principals; fails closed without a verifier; no bypass is to be added |
| Spam / resource exhaustion | Tonic defaults; admission control happens in `Node::submit_transaction` before consensus (rate shaping is a known gap — pre-release, documented) |
| Private-payload leakage via org-gated paths | Fail closed: unverifiable organizations stay connected but every org-gated path rejects (downgrade, not disconnect) |

### 3.3 Storage and state

| Threat | Evidence |
|---|---|
| Ledger tampering | SHA-256 chained blocks; any mutation breaks the chain and is detected on replay/sync |
| State/automation divergence after restart | Contract and watcher state are **rebuilt by replaying the committed chain** — no side state can silently drift |
| Private keys exfiltrated via storage copies | Identity material is an **operator-owned file** (ADR-018), never in the storage seam; backups are scrubbed via `glasschain backup-scrub` |

### 3.4 WASM contract execution

| Threat | Evidence |
|---|---|
| Sandbox escape / unbounded execution | wasmtime sandbox; fuel metering (`GasCosts`/`GasCounter`) bounds work; no `unsafe` in workspace crates (`unsafe_code = "deny"`) |
| Crypto-backend compromise | BLS via `blst` C backend behind an audited, allow-listed boundary (ADR-015) |

### 3.5 Consensus (finality)

| Threat | Evidence |
|---|---|
| Cross-chain / cross-height / cross-round / cross-phase vote replay | **Context-authenticated votes**: signature binds `domain ‖ genesis-hash ‖ height ‖ round ‖ phase ‖ block-hash` ([CONTEXT.md](../CONTEXT.md)) |
| Byzantine equivocation | **Equivocation proofs** are recorded as advisory evidence — detection never auto-excludes; exclusion is an operator decision |
| Quorum forgery | BLS12-381 aggregate multisig with sum-of-keys PoP (ADR-014); ⅔+ validator-set threshold; validators are member organizations (full participation in v1) |

## 4. Out of scope / accepted limitations (pre-release, honest)

These are **not** mitigated and must not be silently "fixed":

- **TOFU is address-bound** — a pin is keyed by advertised address; losing a
  pinned identity key needs operator recovery (delete the `tofu:peer:<addr>`
  state key). Rotation is signed by the pinned key only.
- **Trust-store distribution is manual and out-of-band** between organizations
  — no shared CA, no on-chain registry (deferred, #74).
- **Single-maintainer bus factor** — see [GOVERNANCE.md](../GOVERNANCE.md).
- **No multi-year CVE triage history** — pre-release status; CVD SLAs are
  policy commitments ([SECURITY.md](../SECURITY.md)), not historical evidence.
- Endorsement enforcement waits on capability activation; the SDK does not
  speak gRPC yet; the libp2p swarm path is unwired (TCP+TLS is the shipped
  transport) — [docs/architecture.md](architecture.md) §7.

## 5. Assurance case — claims to evidence map

| Security claim | Evidence |
|---|---|
| Peer transport is encrypted and authenticated | TLS 1.3 default; fingerprint verification on `Hello`; TOFU pins persisted; possession proofs session-bound |
| Only issued, unrevoked member identities act on org-gated paths | `CertChainVerifier` + fail-closed CRL + OCSP staple verification (ADR-011/013/017) |
| Committed history is tamper-evident | SHA-256 chain; replay rebuild; adversarial sync paths covered by network integration and chaos tests |
| Finality is achieved by ⅔+ of distinct validator keys over context-bound votes | BFT round machine + context-authenticated votes + BLS aggregate certificates (ADR-002/014, `crates/glasschain-core/src/bft.rs`) |
| Untrusted code cannot exceed granted resources | wasmtime + fuel metering, contract tests in `glasschain-vm` |
| Automated regression security | CI gates: rustfmt, clippy `-D warnings` (all/pedantic/nursery), full test matrix on 3 OSes, coverage thresholds ≥90% line / ≥80% branch, weekly `cargo audit` (RustSec), Dependabot, fuzz smoke on PRs (`fuzz-wire`, `fuzz-transactions`) |
| Untrusted decode surfaces resist malformed input | `cargo-fuzz` harnesses over `Message` and `Transaction` decode, run on schedule + PR smoke |

Verification pointers for reviewers: the per-claim verification map in
[docs/privacy-and-identity.md](privacy-and-identity.md) (Appendix) and the
integration test suites in `crates/glasschain-network/tests/`.
