# Plan — Tendermint-class BFT behind the consensus seam (ticket #42)

**Ticket:** [#42 Tendermint-class BFT behind the consensus seam](https://github.com/dbbvitor/GlassChain/issues/42)
**Gates:** the four ADR-010 adoption gates stand — this ticket delivers the
staged, **default-off** engine and its activation wiring, **not** a production
testnet adoption.

## Reading of the ACs (SEA committed to)

Research (`bft-at-scale.md`) + ADR-010: Malachite stays a contextional, gated
candidate; **no alpha engine is integrated**. The literal AC deliverable is a
Tendermint-class `ConsensusProvider` that hands every commit consumer a **real**
(verifiable, non-degenerate) quorum certificate — the seam guarantee ADR-002
deferred to this ticket — with activation bound to the already-registered
`bft_consensus` capability ([#36] left it future-height-activatable but inert).

The current `ConsensusProvider` seam is **synchronous and single-shot**
(propose → block+cert). Real multi-process BFT vote rounds are async network
work, which is exactly what the testnet gate defers. Scoped here: a
self-contained BFT provider whose `propose_block` yields a block over which
ed25519 attestations from a configured validator set form a genuine ⅔+ quorum
certificate, and whose `validate_block` **cryptographically verifies** that
quorum. The remote vote-gathering protocol is noted as the testnet gate.

## Changes

### 1. `glasschain-core/Cargo.toml`
- Add `ed25519-dalek = { version = "3.0", features = ["alloc"], optional = true }`.
- Add feature `bft = ["dep:ed25519-dalek"]`, **default stays empty** (default-off).

### 2. `glasschain-core/src/bft.rs` (new, `#[cfg(feature = "bft")]`) — as shipped
- `ValidatorInfo { name: String, public_key: [u8;32] }`.
- `BftConsensusProvider { validators: Vec<ValidatorInfo>, signing_key: SigningKey }`.
  - `new(validators, signing_key)` (const).
  - `quorum()` = `validators.len()*2/3 + 1` (⅔+).
  - `attest(block)`: recompute `block.hash` (no PoW), sign it with the local key
    → one `Attestation` (validator name looked up from the set), wrap in a
    `QuorumCertificate`. A produced cert carries exactly one attestation — a
    1-validator set is its own quorum (`ponytail:` marker at the site).
  - `verify_certificate(cert, block)`: structural check + reject degenerate +
    unknown-validator fail-closed + per-attestation ed25519 verify over
    `block.hash` + ≥ quorum **distinct** keys (HashSet dedups).
  - `validate_block` (trait): structural `chains_to` only — the seam hands no
    certificate.
  - `propose_block` (trait): `with_write_set` + `attest` + notification
    `validate()`.
  - `name()` = `"tendermint-bft"`.
- Re-export via `lib.rs` `#[cfg(feature="bft")] pub use bft::...`.

### 3. `glasschain-core/src/lib.rs`
- `#[cfg(feature = "bft")] pub mod bft;` + `pub use`.
- `ConsensusProvider::propose_block`/`validate_block` stay the same → **swapping
  engines changes no commit consumer** (AC3).

### 4. Node seam routing (AC3/AC4) — as shipped
- `NodeState` gains a feature-gated `consensus: Option<Arc<BftConsensusProvider>>`
  (concrete type, not the trait: the seam's sync `propose_block` cannot express
  the node's pre-computed write set, and there is exactly one BFT impl — promote
  to a trait method when a second engine arrives).
- `Node::set_bft_consensus` attaches it; `mine_async` branches: provider present
  **and** `bft_consensus` active at the candidate height → `provider.attest`
  (real cert, no PoW); otherwise unchanged PoW. The commit consumer
  (`commit_mined_block` → `after_block_commit` → event/broadcast) is identical
  for both arms.
- `pub const BFT_CONSENSUS_CAPABILITY_ID` added to `capability.rs` so the
  node gate and registry entry cannot drift.
- Known staged ceilings (documented in README): `try_replace_chain` (sync) and
  `restore_ledger`/`validate_chain` (restart) stay PoW-coupled; certificates are
  not persisted with blocks, so BFT verification after restart needs the
  adoption-gate design work.

### 5. Wire version + README
- Bump `PROTOCOL_VERSION` to reflect the BFT block (attested) path per AC4.
- README: BFT is staged, default-off, with the four adoption gates; PoW remains
  default. PLUGIN_KIT.md note if `ConsensusProvider` doc changes.

### 6. Tests
- Core unit tests for `BftConsensusProvider`: real quorum cert produced;
  `validate_block` accepts ⅔+ genuine signatures; rejects wrong signature,
  under-quorum, duplicate-validator. Feature-gated to `bft`.
- Node-level no-fork/final-at-commit scenario (network integration or core):
  BFT commit notification's certificate validates against the committed block
  → final at commit.

## Out of scope (deferred to the adoption gates)
- Real multi-process vote gossip / network round protocol.
- Malachite/tendermint-rs engine integration.
- Validator-set churn / per-height set change.
