# Plan — Quorum certificate on the consensus seam and no-fork semantics (ticket #38)

**Ticket:** [#38 Quorum certificate on the consensus seam and no-fork semantics](https://github.com/dbbvitor/GlassChain/issues/38)
**Spec:** ADR-002 (resolved) · spec decision 6 · handoff "Stage 2"

## Scope

1. **`glasschain-core/src/consensus.rs`** — identity-neutral `Attestation`,
   `QuorumCertificate` (index/hash + attestation set; `pow()` degenerate
   certificate; structural `validate()`), and `CommitNotification { block,
   certificate }` (`for_pow_block`, `validate()`).
2. **`ConsensusProvider::propose_block`** now returns `CommitNotification` —
   commit consumers receive the attestation set from the seam and never depend
   on "the leader said so". `PowConsensusProvider` supplies the degenerate
   certificate (PoW's attestation is the valid nonce, carried by the block).
3. **Node** — `NodeEvent::BlockMined`/`BlockReceived` carry the certificate;
   `mine_async` (dev/test consensus driver, retained per ADR-002) builds the
   notification; received PoW blocks derive the degenerate certificate.
4. **Retire mining surfaces** — REPL `mine`/`mine-async` commands and the
   `MineBlock` gRPC method (+ proto, server, integration tests); internal
   `Node::mine()` stays as the dev/test driver. README updated.
5. **Fork-asserting tests rewritten** — `test_concurrent_mining_longest_chain_wins`
   and `test_madsim_application_layer_partition_and_merge` become
   finality/liveness assertions: every committed block carries a certificate
   that validates at commit, and syncing nodes converge.
6. **Node-level scenarios** — a block is final at commit (certificate validates
   against the committed block) and a verifying member validates the attestation
   set on receipt.

## Out of scope

Real BFT attestations, quorum verification crypto, and the wire transport of
certificates (ticket #42 — PoW certs are derivable from the block itself, so
the `Message::Block` wire stays unchanged; BFT introduces the attested block
message).
