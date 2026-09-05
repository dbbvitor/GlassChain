# Plan — Post-quantum exposure and crypto agility

**Status:** current 2026-09-05. One action ready (transport, with a verification test);
signature agility shipped, migration deferred; archival-evidence research option open.
No new dependency, issue, or commit required by this plan.
**Relates to:** [ADR-003](../../docs/adr/adr-003-privacy-model.md),
[ADR-008](../../docs/adr/adr-008-endorsement-policy-model.md),
[ADR-002](../../docs/adr/adr-002-consensus-finality.md),
[ADR-014](../../docs/adr/adr-014-bls-aggregated-certificates.md),
D5 ([`deferred-code-debt.md`](deferred-code-debt.md))
**Evidence:** [`../memories/post-quantum-research.md`](../memories/post-quantum-research.md)
(§1–§7 + Follow-ups 1–3); RFC 3161/4998/6283 and FIPS 203/204/205 re-read 2026-09-05.

---

## Verdict

Three assets fail differently; conflating them turns this into a rewrite nobody needed:

| Asset | Exposure | Clock | Posture |
|---|---|---|---|
| **Transport confidentiality** | Shor breaks X25519; recorded traffic decrypts retroactively | Harvest-now-decrypt-later — **now** | Runtime provider → `aws-lc-rs` rustls, negotiate hybrid `X25519MLKEM768`, prove it in a test (§2) |
| **Signatures** (Ed25519 app, BLS QCs, imported enterprise PKI) | Shor threatens these classical public-key schemes | No guaranteed date — below | Discriminants shipped; migration needs a risk/requirement trigger (§3) |
| **Hashing** (SHA-256) | Grover, heavily discounted; collision search Category 2 | None | **No action** (§4) |

Corrections to the previous edition (Follow-up 3 supersedes):

- **Not "compliant today."** Catalogue membership (DOC-ICP-01.01 v6.0 added ML-DSA/ML-KEM
  as *support*) is not compliance; compliance is a property of specific requirements
  under a certificate policy.
- **2035 is a draft proposal, not a prediction** (IR 8547 ipd dates NIST's own FIPS; not
  a quantum-computer guarantee, not a legal deadline). No "pre-2035 safe" claims.
- **Algorithms and on-chain hashes give technical integrity, not compliance or proof
  weight** (§4, §7).

---

## 1. What we actually run

Verified against the tree, 2026-09-05:

- **Peer TLS:** rustls 0.23 (locked 0.23.43), `features = ["ring"]` with defaults on —
  **`aws-lc-rs` 1.18.0 / `aws-lc-sys` 0.44.0 are already compiled and locked**. The
  provider is chosen at **runtime, in code**: `Node::build_tls` attempts to install
  the ring default (and ignores installation failure); `AcceptAnyCert` also names
  ring signature algorithms. Configs use the installed process default. **No test
  pins which provider or KX group any path negotiates — verify, do not assume.**
- **Cert verification:** `glasschain-identity` — `rustls-webpki` with the `ring` feature
  for signature algorithms (P-256/P-384/ed25519); unaffected by the KX change; PQ
  *certificate* signatures are separate, deferred.
- **Signatures:** ed25519 (`ed25519-dalek` 3.0) for identities, endorsements, canonical
  records; **BFT** (`bft` feature, default-off): BLS12-381 for votes and quorum
  certificates only (ADR-014). **Hashing:** SHA-256. **Certs:** rcgen 0.14, self-issued
  ed25519 org roots. **libp2p:** Noise `XX`/X25519, pinned 0.56.0 — not the active
  transport.

## 2. First priority: transport confidentiality

Recorded classical peer traffic, including PDC payloads (ADR-003), may become
readable to a sufficiently capable quantum adversary. Prioritize hybrid key
exchange for data whose confidentiality must outlast the migration window.
Archive evidence also needs action before its trust mechanisms fail (§7).

**Fix shape: runtime provider selection, not a new dependency.** Hybrid
`X25519MLKEM768` ships only in rustls' `aws-lc-rs` provider; `ring` has no post-quantum
group (research §1). `aws-lc-rs` is already in the lock, so this is a **provider
switch**: `build_tls`'s `install_default` + the three ring references in `node.rs` +
the two in tests, with consistent feature lines (identity's webpki `ring` signature
algorithms unchanged). **Verification, not assumption, is the change:** a two-node test
asserts the negotiated group is `X25519MLKEM768` (non-supporting peers fall back to
X25519 — no wire break) plus the four gates on Ubuntu, macOS, Windows. **Do not
hand-roll a KEM, add a PQ crate, or design a custom hybrid handshake.**

**Cost, uncoupled from a blanket C debate:** aws-lc-sys builds via `cmake`; ring and
wasmtime already carry C/C++ builds in this graph, so the "audited C backends?"
question (shared with `blst`, issue #85) is **not a blanket new blockade** — per
backend, on measured need. The real risk is the CI build.

**libp2p:** no action — classical Noise/X25519 has no shipping PQ option (upstream
`mlkem-hfs` PR open). Do not promote libp2p to the active transport without revisiting.

## 3. Signatures: agility shipped, migration not done

**Agility is shipped; the migration is not.** Since 2026-09-03 every signature carrier
carries `core::wire::SignatureAlgorithm` — ed25519 default (omitted on the wire; legacy
JSON still parses), unknown discriminant a deserialization error, never silent ed25519
(test-verified): `RecordSignature`, `EndorserIdentity`, `BftVote`/`QuorumCertificate`
(currently `Bls12381`), `ValidatorInfo.public_key: Vec<u8>`. A future migration
still requires implementation, key/certificate profiles, size limits, rollout and
historical verification—not just a wire-version bump. **The shipped schemes remain ed25519
(application) and BLS12-381 (quorum certificates) — no post-quantum scheme is in use
anywhere.**

**ML-DSA adoption stays deferred.** Sizes (FIPS 204 Table 2): pk 1 312 B / sig 2 420 B
(ML-DSA-44) ≈ 41×/38× ed25519 — a 201-attestation quorum would be ~487 KB vs ~13 KB
(§5). Trigger: a **named requirement** (an ICP-Brasil policy level mandating ML-DSA —
not catalogue membership — or a customer/product requirement), not a date.

**The forgery claim, corrected:** forgery capability does not automatically alter
copies already held by honest participants, but can undermine classical evidence
and fabricate apparently valid historical signatures/certificates. Hash links alone
do not authenticate an alternative history to a new or recovering verifier.
Trusted checkpoints, historical validator-set validation and renewable archive
evidence matter; neither replication nor a calendar date makes signatures safe.

## 4. Hashing: no action — and no implied guarantees

NIST IR 8105: hash functions "should be usable in a quantum era"; the full Grover
speedup needs serial execution (Zalka; NIST PQC FAQ), parallelism buys ≈√M
(Bernstein), and Grover does not apply to *collision* resistance. NIST IR 8547 puts
SHA-256 collision search at Category 2. **SHA-256 stays.** And an on-chain hash is
technical evidence, not legal proof — no algorithm or hash choice is a compliance
guarantee by itself.

## 5. BLS and the performance plan

BLS aggregation is **shipped** (ADR-014) for quorum certificates. It is
pairing-based and vulnerable to Shor's algorithm; no guaranteed useful lifetime
through 2035 follows. Discriminants help distinguish formats but do not implement
a migration. Do not assume PQ signatures can retain BLS's size/verification costs.

## 6. What this is not

Not a reason to add a ZK identity stack, reopen ADR-002, or make a "quantum-resistant
blockchain" positioning claim (transport, BLS, and ed25519 are all classical until each
is migrated). Not a justification for a pre-emptive ML-DSA migration this year — ask
which requirement it serves.

## 7. Long-term archival evidence — RFC 4998/3161 Merkle research (optional)

The established answer to long-lived non-repudiation of *classical* signatures is
ERS-style off-chain archival evidence (RFC 4998 makes the "sign a Merkle root over
legacy signatures" idea standard), not changing ledger signatures. **Research/design
option, not adoption**: off-chain, no new dependency, no issue until profile/legal
review says otherwise.

Elements to validate in that review (read 2026-09-05):

- **Archived objects and validation material** — preserve the exact signed
  bytes, signature algorithm/context and original signatures, plus trust anchors,
  certificates, revocation evidence and historical policy. Bind the objects into
  the evidence construction; ERS `cryptoInfos` is not itself timestamp-protected,
  so its contents require independent authentication (RFC 4998 §3.1).
- **Trusted timestamp** — a TSA signs a hash imprint with a key reserved for
  time-stamping; `id-kp-timeStamping` EKU MUST be critical (RFC 3161 §2.3); tokens
  SHOULD be re-stamped before the TSA key's lifetime ends (§4).
- **Inclusion proof** — reduced Merkle tree over the archived objects, timestamped only
  at the root, per-object proofs (RFC 4998 §1.2/§4.2).
- **Renewal before compromise** — hash-tree renewal re-hashes/re-timestamps prior chains
  (RFC 4998 §5); evidence outliving a *publicly* weakened algorithm may lose probative
  force — ERS cannot retroactively fix it (§1.1/§7, RFC 6283 §9.2).
- **ML-DSA vs SLH-DSA — comparison only:** evaluate ML-DSA as the primary
  candidate given the catalogue recorded in earlier research; compare SLH-DSA's
  hash-based assumptions and parameter-dependent key/signature sizes for a
  supplementary archive profile. Neither is selected here. NIST standardization
  is not ICP profile approval, and absence from an ICP catalogue is not a blanket
  prohibition on supplementary private evidence. Confirm current profile, TSA,
  interoperability and legal requirements before choosing.
- **Retention vs legal hold (debt D5):** archival evidence preserves (legal hold), the
  opposite interest from D5's privacy-purge duty
  ([`deferred-code-debt.md`](deferred-code-debt.md)). An evidence scheme does not
  satisfy D5's deletion controls, and D5's 72h retention is not evidence retention;
  define each with its own retention and copy semantics.

## 8. HSM and password KDFs

Corrected framing (Follow-up 3):

- **HSM is not an automatic requirement for every separate runtime key.** DOC-ICP-04
  v8.3 Tabela 4 mandates hardware for A3/A4/SE-H/T3/T4/AE-H/OM-BR; **SE-S and AE-S
  permit a software repositório.** The duty binds only once node identity is bound to
  ICP-Brasil credentials of the hardware types — not today, not per-key.
- **Argon2 is a password KDF, not post-quantum cryptography** — it stretches a
  low-entropy secret and is a candidate for nothing in this plan. Its only home is an
  operator-facing software keystore (SE-S/AE-S tier or dev), where Argon2id would be the
  right KDF — not on any current path, not required here.

---

## Ordered actions

1. **Transport provider switch (ready):** runtime provider → `aws-lc-rs` (`build_tls`
   `install_default`, three ring references in `node.rs`, two in tests), consistent
   feature lines, and a negotiated-group test (`X25519MLKEM768`) on the two-node path.
   Four gates green on Ubuntu, macOS, Windows. No new dependency (aws-lc-rs already
   locked); the C-backend question shared with issue #85 is per-backend, not a blanket
   blockade.
2. ~~Add an algorithm discriminant / widen `ValidatorInfo.public_key`~~ **Done
   2026-09-03** (§3). The ML-DSA *migration* stays deferred behind the §3 trigger.
3. **Libp2p module note:** promoting it to the active transport requires revisiting PQ
   key exchange.
4. **Archival-evidence research (§7):** identify retention lifetime, relying parties,
   trusted time source, available PQ-capable TSA/profile and renewal owner; compare
   ML-DSA/SLH-DSA batching off-chain. Keep originals and proof material in authorized
   storage; a public digest is neither encryption nor anonymization. No consensus
   dependency, changed `SCHEMA_V1`, mandatory external storage service or automatic
   adoption. Profile/legal review and independent verification gate implementation.
5. **Nothing for SHA-256.**

## Validation

- Action 1: negotiated-group test on the two-node path + four gates on three CI
  platforms (aws-lc-sys build is the real risk).
- Action 2 (shipped): unknown-discriminant rejection round-trip test in `wire.rs`.
- Before implementing §7, specify negative checks for modified original bytes,
  substituted signature/context, invalid/missing inclusion proof, untrusted TSA,
  missing revocation evidence and expired/unrenewed evidence. Verify renewal with
  an independent verifier and measure batch size, proof bytes, signing cost and
  archive lag. A TSA outage must not block consensus; evidence remains visibly
  pending and retries are bounded/idempotent. A late PQ signature cannot repair
  evidence forged before anchoring or retroactively certify its alleged date.

## Out of scope

- Implementing any post-quantum algorithm ourselves, in any form.
- Adopting a PQ signature scheme before a §3 trigger or §7 profile/legal review.
- Changing the hash function (§4); post-quantum for libp2p noise (§2).
- Argon2/scrypt/PBKDF2 unless an operator-facing software keystore is ever built (§8).
- Persistent node identity custody — separate from PQ migration. Certificate-bound
  principals ([identity decision](https://github.com/dbbvitor/GlassChain/issues/87),
  D4) and durable peer pins are related but do not by themselves deliver key storage.
- Claiming compliance, "pre-2035 safe", or proof value from algorithm/hash choice.

## Sources

Primary sources with URLs and read dates are in
[`../memories/post-quantum-research.md`](../memories/post-quantum-research.md) — rustls
releases 0.23.0–0.23.43 + provider docs; FIPS 203/204/205 + FIPS 206 status; NIST IR
8547 ipd; NIST IR 8105, PQC FAQ, Bernstein (SHARCS 2009); ITI DOC-ICP-01.01 v6.0 /
DOC-ICP-04 v8.3 / IN 35/2026; libp2p noise; RFC 3161 / 4998 / 6283; Follow-ups 1–3.
FIPS 204 Table 2 and FIPS 205 re-checked 2026-09-05.

**Still unverified — do not cite as settled:** FIPS 206/FN-DSA status; a final NIST IR
8547 (only the Nov 2024 ipd found); ITI public consultations (JS-rendered); any
ITI/ICP-Brasil recognition of RFC 3161/4998 or ERS (policy question for §7); pre-2024
A1/S1–S4 key-storage wording (does not affect §8, which rests on current DOC-ICP-04
v8.3 Tabela 4).