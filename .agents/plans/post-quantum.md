# Plan — Post-quantum exposure and crypto agility

**Status:** assessment complete; one action ready, one decision deferred, one ruled out
**Date:** 2026-09-02
**Relates to:** [ADR-003](../../docs/adr/adr-003-privacy-model.md) (PDC confidentiality),
[ADR-008](../../docs/adr/adr-008-endorsement-policy-model.md) (MSP identity),
[ADR-002](../../docs/adr/adr-002-consensus-finality.md) (attestation signatures)
**Evidence:** [`../memories/post-quantum-research.md`](../memories/post-quantum-research.md)
— primary sources, read 2026-09-02

---

## Verdict

**Yes, we are vulnerable — but only one of the three exposures has a deadline, and
the fix for it is a dependency feature flag we are already paying for.**

The three assets fail differently, and conflating them is how this topic turns
into a rewrite that nobody needed:

| Asset | Quantum exposure | Deadline | Action |
|---|---|---|---|
| **Peer transport confidentiality** | Shor breaks X25519; recorded traffic is decryptable retroactively | **Now** — harvest-now-decrypt-later | Switch rustls to `aws-lc-rs` (§2) |
| **Signatures** (ed25519, RSA at the ICP boundary) | Shor breaks both, but forgery is not retroactive | 2035 (NIST IR 8547 ipd) | Type-level agility now, algorithms later (§3) |
| **Hashing** (SHA-256) | Grover, heavily discounted | None | **None.** Do not touch (§4) |

**The thing that changed while we were not looking: ICP-Brasil has already moved.**
DOC-ICP-01.01 v6.0 (Instrução Normativa ITI nº 35, 30.01.2026) added ML-DSA and
ML-KEM to the ICP-Brasil algorithm catalogue. Read this precisely: it
*acrescenta suporte* — adds support. Classical RSA-2048, Ed25519 and Curve25519
remain listed for end-user keys, so **we are compliant today and nothing is
burning.** But the direction of travel in our own jurisdiction is now a published
normative fact rather than a guess, and our plans should stop treating
post-quantum as somebody else's future problem.

---

## 1. What we actually run

Measured against the tree, 2026-09-02:

- **Peer TLS:** `rustls 0.23` with `features = ["ring"]`
  (`crates/glasschain-network/Cargo.toml:33`). `glasschain-identity` deliberately
  selects `ring` for `rustls-webpki` too, with a comment saying it does so to
  share one backend with the network crate.
- **Consensus signatures:** `ed25519-dalek 3.0`, feature-gated behind `bft`.
- **Hashing:** `sha2 0.11`, SHA-256 throughout.
- **Certificates:** `rcgen 0.14`, self-issued ed25519 org roots.
- **libp2p:** Noise `XX` over X25519 — but the module doc states this is not the
  active node transport yet.

## 2. The one real exposure: transport confidentiality

This is the only asset where waiting costs something irreversible. An adversary
recording peer traffic today decrypts it the day a cryptographically-relevant
quantum computer exists. PDC payloads move point-to-point over this transport
(ADR-003), so the content is exactly the commercially-sensitive material the
privacy model exists to protect.

**The irritating part: we pinned a rustls that supports the fix and then selected
the provider that cannot do it.**

- Hybrid `X25519MLKEM768` is in rustls' *default* key-exchange groups under the
  **`aws-lc-rs`** provider, from 0.23.22 (2025-01-30), and the
  `prefer-post-quantum` feature has been on by default since 0.23.27
  (2025-05-05).
- Under **`ring`** there is **no post-quantum key exchange at all** — the
  provider exposes only SECP256R1, SECP384R1 and X25519.
- Our `Cargo.lock` resolves rustls to a version with the capability. The feature
  selection is what excludes it.

So the change is: swap the provider feature in `glasschain-network` and
`glasschain-identity` together (they are coupled on purpose), and re-point the
four `rustls::crypto::ring::default_provider()` call sites in `node.rs` (lines
145, 159, 164, 654) plus the two in `tests/`. Hybrid key exchange then negotiates
by default, and a peer that does not support it falls back to X25519 — no wire
break, no interop cliff.

**Do not** hand-roll a KEM, add a PQ crate directly, or design a custom hybrid
handshake. The dependency does this correctly and we would not.

Cost to check before committing: `aws-lc-rs` builds a vendored C/assembly library
via `cmake`, whereas `ring` is a lighter build. That is the real trade, and it is
a CI-time question, not a security one. It must be verified on all three CI
platforms (Ubuntu, macOS, Windows) before this is called done.

**libp2p:** no action. Noise-over-X25519 has no shipping PQ option (a hybrid
`mlkem-hfs` PR is open and unmerged upstream), and our libp2p path is explicitly
not the active transport. The correct handling is a note that says *do not
promote libp2p to the active transport without revisiting this*, not a fix.

## 3. Signatures: buy agility now, buy algorithms later

**Signature forgery is not retroactive.** A quantum adversary in 2035 who can
forge ed25519 cannot rewrite a chain that is hash-linked and replicated across
hundreds of validators — they can only forge *new* signatures going forward. That
is a live-system problem with a decade of warning, not a stored-data problem.
NIST IR 8547 (initial public draft) disallows EdDSA, ECDSA and RSA at ≥128-bit
security **after 2035**, with no prior deprecation step.

The residual risk is narrower and worth naming: **long-lived non-repudiation.** A
record signed under ICP-Brasil in 2026 and challenged in a court in 2040 needs its
signature to still mean something. The established answer to that is trusted
timestamping and re-signing of archives, not switching the consensus signature
algorithm today.

### What to do now: stop hardwiring the algorithm into the types

This is the cheap, high-value item, and it is not cryptography — it is a type
change. Today nothing in the wire format identifies which algorithm produced a
signature:

- `Attestation.public_key` / `.signature` (`core/src/consensus.rs:19`) — bare byte
  vectors; ed25519 is implied by a doc comment.
- `RecordSignature.signature_bytes` (`core/src/canonical.rs:35`) — same.
- `EndorserIdentity.public_key` (`core/src/endorsement.rs:248`) — same.
- **`ValidatorInfo.public_key` is `[u8; 32]`** (`core/src/bft.rs:31`) — a
  fixed-size array that *cannot physically hold* an ML-DSA-44 public key (1312
  bytes). This one is a type-level lock-in, not just a missing label.

Without a discriminant, any future algorithm migration is a hard wire break plus
a chain-history reinterpretation problem. With one, it is a version bump.

**Piggyback it on Step 1 of the performance plan.** That step already changes the
wire encoding (JSON is the standing structural tax) and already requires an ADR
for a binary codec. Adding an algorithm discriminant during a break we are taking
anyway costs almost nothing; adding it later costs a second break.

Sizing note for whoever does the performance work: ML-DSA-44 signatures are 2420
bytes against ed25519's 64. At a 201-attestation quorum that is ~487 KB of
certificate versus ~13 KB. **This interacts badly with performance §6 Step 4
(BLS)** — see §5.

### What to defer

Actually adopting ML-DSA. Trigger: ICP-Brasil making it mandatory rather than
merely supported, or a customer requirement. Not before. Adopting a signature
scheme with a 38× size penalty ahead of any requirement, in a system whose stated
sell factor is latency and scalability, would be trading our differentiator for a
threat that has a 2035 date on it.

## 4. Hashing: no action, and resist the urge

The "Grover halves SHA-256 to 128 bits" line is a conservative upper bound that
the literature does not take at face value:

- NIST IR 8105 says hash functions "should be usable in a quantum era" and that
  doubling output size "may be overly conservative."
- The NIST PQC FAQ notes the full quadratic speedup requires Grover run *in
  series* (Zalka); parallel machines diminish it, and variants "will provide no
  advantage" for attacks that must complete in years or decades.
- Bernstein's cost analysis: a parallel size-M quantum machine gains only ~√M —
  no better than M classical machines, before communication costs.
- For **collision** resistance — the property a hash-linked chain actually
  depends on — Grover does not apply directly at all.

NIST IR 8547 places SHA-256 collision search at Category 2. SHA-256 stays.

## 5. The uncomfortable interaction with the performance plan

**Performance §6 Step 4 promotes BLS aggregation, and BLS is pairing-based, which
means Shor breaks it too.** There is no post-quantum aggregate signature scheme
with comparable size and verification properties available today.

This does not cancel Step 4 — the light-client ladder argument that promoted it
stands, and the 2035 horizon gives it a full useful life. But it does mean:

- **Do not market BLS aggregation as future-proofing.** It is a 2026–2035
  optimization.
- The algorithm discriminant from §3 becomes *more* important, not less, because
  a BLS adoption is a second signature scheme in the wire format before any
  post-quantum one is.

## 6. What this is not

- **Not a reason to add a ZK identity stack.** Already ruled out separately.
- **Not a reason to reopen ADR-002.** The consensus family is orthogonal to the
  signature algorithm.
- **Not a "quantum-resistant blockchain" positioning exercise.** We would be
  claiming a property our transport does not have while our signatures and our
  planned aggregation are both classical.
- **Not urgent for signatures.** Anyone proposing an ML-DSA migration this year
  should be asked which requirement it serves.

---

## Ordered actions

1. **Swap rustls to `aws-lc-rs` in `glasschain-network` and `glasschain-identity`
   together**, verify the build on all three CI platforms, and confirm the
   negotiated group in a test. Closes the only harvest-now-decrypt-later
   exposure. *Ready to start.*
2. **Add an algorithm discriminant** to `Attestation`, `RecordSignature` and
   `EndorserIdentity`, and widen `ValidatorInfo.public_key` from `[u8; 32]` to a
   variable-length representation. **Fold into performance Step 1**, not a
   separate wire break.
3. **Note in the libp2p module docs** that promoting it to the active transport
   requires revisiting PQ key exchange, since Noise/X25519 has no shipping option.
4. **Decide on ML-DSA when ICP-Brasil makes it mandatory**, not before. Re-read
   DOC-ICP-01.01 at each ITI Instrução Normativa.
5. **Nothing for SHA-256.**

## Validation

- Action 1: a test asserting the negotiated key-exchange group is
  `X25519MLKEM768` between two GlassChain nodes, plus the four workspace gates
  green on Ubuntu, macOS and Windows (the `aws-lc-rs` build is the actual risk).
- Action 2: a round-trip test proving an unknown algorithm discriminant is
  rejected rather than silently treated as ed25519.

## Out of scope

- Implementing ML-DSA or ML-KEM ourselves in any form.
- Post-quantum signatures in consensus (§3, deferred with a named trigger).
- Changing the hash function (§4).
- Post-quantum for libp2p noise (§2 — nothing to adopt upstream).

## Sources

See [`../memories/post-quantum-research.md`](../memories/post-quantum-research.md)
for primary sources with URLs and read dates: rustls release notes and docs.rs,
NIST FIPS 203/204/205, NIST IR 8105 / IR 8547 (ipd), NIST PQC FAQ, Bernstein
(SHARCS 2009), ITI DOC-ICP-01.01 v6.0 and Instrução Normativa ITI nº 35, and the
rust-libp2p repository.

**Flagged as unverified in that research:** FIPS 206 / FN-DSA status (no CSRC
record), a *final* NIST IR 8547 (only the November 2024 initial public draft was
found), and an exhaustive search of ITI public consultations. Do not cite those
three as settled.
