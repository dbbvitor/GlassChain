# Post-Quantum Readiness — Primary-Source Research

**Learned:** 2026-09-02

Date-read note: every URL below was read on **2026-09-02** unless otherwise noted.
This file is the durable record for GlassChain's post-quantum readiness assessment; it is
fact-only, with each claim tied to a primary source. Anything not verifiable is flagged.

---

## 1. rustls post-quantum key exchange

**Facts**

- rustls 0.23.0 (released 2024-02-29) made **aws-lc-rs** the default crypto provider; `ring`
  continued as a crate feature (`rustls = { features = ["ring"] }`).
  Source: https://api.github.com/repos/rustls/rustls/releases (release bodies, read 2026-09-02);
  https://github.com/rustls/rustls/releases/tag/v/0.23.0
- Hybrid post-quantum KX timeline (release notes, same API source):
  - 0.23.2 (2024-03-13): groundwork; experimental `X25519Kyber768Draft00` shipped in the
    separate `rustls-post-quantum` 0.1.0 crate.
  - `rustls-post-quantum` 0.2.0 (2024-12-11): moved to standardized **X25519MLKEM768**;
    **removed** X25519Kyber768Draft00 (breaking change).
  - rustls 0.23.22 (2025-01-30): native **X25519MLKEM768** support, **aws-lc-rs provider only**;
    "supported by default, but offered at a low algorithm negotiation priority"; new
    `prefer-post-quantum` crate feature reorders it to highest priority; maintainers said they
    expected to add it to default features "in a future minor release".
  - rustls 0.23.27 (2025-05-05): "Prefer post-quantum key exchange algorithms by default"
    (`prefer-post-quantum` added to **default features**, PR #2425).
  - rustls 0.23.28 (2025-06-16): `secp256r1mlkem768` added but **not offered by default**
    (opt-in via a custom `CryptoProvider::kx_groups`).
  - rustls 0.23.37 (2026-02-24): ML-KEM-1024 key exchange support (in the aws-lc-rs provider).
  - Latest release: 0.23.43 (2026-07-29).
- Provider kx-group lists (docs.rs rustls 0.23.43, read 2026-09-02):
  - `rustls::crypto::ring::kx_group`: **SECP256R1, SECP384R1, X25519 only — no post-quantum
    group at all.**
    https://docs.rs/rustls/latest/rustls/crypto/ring/kx_group/index.html
  - `rustls::crypto::aws_lc_rs::kx_group`: MLKEM768, MLKEM1024, SECP256R1,
    SECP256R1MLKEM768, SECP384R1, X25519, X25519MLKEM768.
    https://docs.rs/rustls/latest/rustls/crypto/aws_lc_rs/kx_group/index.html
- Defaults rationale manual: X25519MLKEM768 "is available when using the aws-lc-rs provider.
  The `prefer-post-quantum` crate feature makes X25519MLKEM768 the highest-priority key
  exchange algorithm. Otherwise, it is available but not highest-priority." Pure `MLKEM768`
  "is not currently enabled by default out of conservatism."
  https://docs.rs/rustls/latest/rustls/manual/_05_defaults/index.html
- Post-quantum **signatures** are separate and still experimental: `rustls-post-quantum`
  0.2.3 (2025-07-16) ML-DSA *verification* and 0.2.4 (2025-09-23) ML-DSA *signing*, both
  behind the `aws-lc-rs-unstable` feature.

**Answers**

- Hybrid PQ KX: yes — X25519MLKEM768 (and, post-0.23.28, secp256r1mlkem768; post-0.23.37,
  ML-KEM-1024) via the **aws-lc-rs provider only**.
- From which version: core rustls 0.23.22 (2025-01-30); standardized X25519MLKEM768 existed
  in `rustls-post-quantum` 0.2.0 from 2024-12-11.
- Default vs opt-in: in 0.23.22+ X25519MLKEM768 is present in the aws-lc-rs provider's
  DEFAULT_KX_GROUPS at low priority; since 0.23.27 the `prefer-post-quantum` feature is in
  **default features**, so PQ is effectively preferred by default for aws-lc-rs users.
- Under `ring`: **no post-quantum key exchange exists**. None in `crypto::ring::kx_group`,
  none offered.
- GlassChain relevance (repo evidence): `Cargo.lock` pins rustls **0.23.43** (also
  aws-lc-rs 1.18.0 via default features, ring 0.17.14);
  `crates/glasschain-network/Cargo.toml` selects `rustls = { version = "0.23", features =
  ["ring"] }`. The project therefore runs the `ring` provider → **no PQ KX available to
  GlassChain today even though the pinned version supports it under aws-lc-rs**. (Identical
  choice mirrored in `glasschain-identity` for rustls-webpki.)

---

## 2. NIST post-quantum standards status

**Facts (all read 2026-09-02)**

- **FIPS 203 (ML-KEM)** — Final, published **2024-08-13**; DOI 10.6028/NIST.FIPS.203.
  https://csrc.nist.gov/pubs/fips/203/final
- **FIPS 204 (ML-DSA)** — Final, published **2024-08-13**; DOI 10.6028/NIST.FIPS.204.
  https://csrc.nist.gov/pubs/fips/204/final
- **FIPS 205 (SLH-DSA)** — Final, published **2024-08-13**; DOI 10.6028/NIST.FIPS.205.
  https://csrc.nist.gov/pubs/fips/205/final
- All three also listed Final/8/13/2024 in the CSRC publications search:
  https://csrc.nist.gov/publications/search?keywords=fips%20206 (search page surfaced the
  203/204/205 records).
- **FIPS 206 / FN-DSA (Falcon): NOT verifiable as published.** No record exists in the CSRC
  publications database ("FN-DSA" search → "No results were found"); the URLs
  csrc.nist.gov/pubs/fips/206/final, /fips/206/ipd, nvlpubs.nist.gov/nistpubs/FIPS/NIST.FIPS.206.ipd.pdf,
  and doi.org/10.6028/NIST.FIPS.206.ipd all 404'd. I therefore **cannot confirm any
  draft/final status** for FIPS 206 from a primary source on the read date.
- Falcon/HQC context (verified): PQC project page (page updated 2026-08-05) states Falcon and
  HQC "were selected for ongoing standardization; that process is underway."
  https://csrc.nist.gov/projects/post-quantum-cryptography
- Status reports in that process (CSRC publications search results, read 2026-09-02):
  IR 8545 (Final, 2025-03-11) "Status Report on the Fourth Round…"; **IR 8610 (Final,
  2026-05-14) "Status Report on the Second Round of the Additional Digital Signature
  Schemes…"**.

---

## 3. NIST migration timeline — NIST IR 8547

**Source:** NIST IR 8547 (Initial Public Draft), November 2024.
https://nvlpubs.nist.gov/nistpubs/ir/2024/NIST.IR.8547.ipd.pdf (text extracted via pdftotext,
read 2026-09-02). A final version at csrc.nist.gov/pubs/ir/8547/final was **not found**
(404 on read date) — treat the dates as the draft's proposal, not final policy. The PQC
project page corroborates the draft's intent: NIST "will deprecate and ultimately remove
quantum-vulnerable algorithms from its standards by 2035".

**Verbatim definitions** (as quoted in IR 8547 §2, from SP 800-131A):

- *Deprecated*: "the algorithm and key length/strength may be used, but there is some
  security risk. The data owner must examine this risk potential and decide whether to
  continue to use a deprecated algorithm or key length."
- *Disallowed*: "the algorithm, key length/strength, parameter set, or scheme is no longer
  allowed for the stated purpose."
- *Legacy use*: "may only be used to process already protected information (e.g., to decrypt
  ciphertext data or to verify a digital signature)."
- *Acceptable*: "approved for use in accordance with any associated guidance."

**Schedules (IR 8547 ipd, Tables 2 & 4):**

| Algorithm family | Parameters | Transition |
|---|---|---|
| ECDSA (signatures) | 112-bit | **Deprecated after 2030; Disallowed after 2035** |
| ECDSA | ≥128-bit | Disallowed after 2035 |
| EdDSA | ≥128-bit | Disallowed after 2035 |
| RSA (signatures) | 112-bit | Deprecated after 2030; Disallowed after 2035 |
| RSA | ≥128-bit | Disallowed after 2035 |
| Finite-field DH/MQV; EC DH/MQV; RSA (key establishment) | 112-bit | Deprecated after 2030; Disallowed after 2035 |
| Same, ≥128-bit | | Disallowed after 2035 |

- Higher-security ≥128-bit classical schemes have **no deprecation step** — they go straight
  to disallowed after 2035.
- IR 8547 notes SP 800-57 Part 1 had projected disallowing 112-bit public-key schemes on
  **January 1, 2031**; NIST now intends **instead to deprecate** (not fully disallow) the
  112-bit level to give migration headroom.
- Symmetric: "all NIST-approved symmetric primitives that provide at least 128 bits of
  classical security are believed to meet the requirements of at least Category 1 security";
  the few 112-bit symmetric standards "will be disallowed in 2030".

---

## 4. Grover's algorithm vs SHA-256

**Facts (primary sources, read 2026-09-02):**

- **NISTIR 8105 "Report on Post-Quantum Cryptography" (April 2016)**:
  "Grover's algorithm provides a quadratic speed-up … We don't know that Grover's algorithm
  will ever be practically relevant, but if it is, doubling the key size will be sufficient to
  preserve security. Furthermore, it has been shown that an exponential speed up for search
  algorithms is impossible, suggesting that **symmetric algorithms and hash functions should
  be usable in a quantum era**." Also: doubling key lengths "may be overly conservative, as
  quantum computing hardware will likely be more expensive to build than classical hardware."
  https://nvlpubs.nist.gov/nistpubs/ir/2016/NIST.IR.8105.pdf
- **CSRC PQC FAQ (page updated 2026-08-05)** — current NIST position:
  - "it was proven by Zalka in 1997 that **in order to obtain the full quadratic speedup, all
    the steps of Grover's algorithm must be performed in series**. In the real world, where
    attacks on cryptography use massively parallel processing, the advantage of Grover's
    algorithm will be smaller… it is quite likely that Grover's algorithm will provide little
    or no advantage in attacking AES, and AES 128 will remain secure for decades to come."
  - "practical attacks typically must be run in parallel on large clusters of machines, which
    **diminishes the speedup that can be achieved using Grover's algorithm**. When all of
    these considerations are taken into account, it becomes quite likely that variants of
    Grover's algorithm will provide no advantage to an adversary wishing to perform a
    cryptanalytic attack that can be completed in a matter of years, or even decades."
  - (Category-2 assumption holds while the adversary is depth-limited to fewer than about
    2^87 logical quantum gates.)
  - Caveat to cite precisely: the FAQ's explicit statements are about symmetric *key search*
    (AES); hash functions are covered by NISTIR 8105's "symmetric algorithms and hash
    functions" statement and by IR 8547's categorization.
  https://csrc.nist.gov/projects/post-quantum-cryptography/faqs
- **NIST IR 8547 ipd, Table 1**: post-quantum Category 2 = "Collision search on a 256-bit
  hash function — SHA-256"; §4.1.3: symmetric ≥128-bit classical ⇒ at least Category 1.
- **D. J. Bernstein, "Cost analysis of hash collisions: Will quantum computers make SHARCS
  obsolete?" (SHARCS 2009)** (well-cited academic source for why the naive figure is
  pessimistic): quantum preimage search ≈ 2^(b/2)·h operations on Θ(h) qubits; a size-M
  *parallel* quantum machine achieves only ≈ 2^(b/2)·h/M^(1/2) — i.e. parallelism buys just
  M^(1/2), "no better than running M parallel copies" of classical search, plus communication
  costs grow by M^(1/2).
  https://cr.yp.to/hash/collisioncost-20090823.pdf

**Consensus statement (grounded in the above):** the naive "SHA-256 preimage ≈ 2^128 under
Grover" is treated by NIST and by the literature as a conservative upper bound on the
attacker's *advantage*, not a real halving of effective security: Grover's quadratic speedup
is inherently serial (Zalka 1997, as cited by NIST), parallelization yields only M^(1/2) at
linear-to-superlinear cost, quantum hardware is far costlier than classical, and NIST states
hash functions should remain usable in a quantum era. NIST still maps SHA-256 collision
search to Category 2 — i.e. it quantifies the bound but does not mark SHA-256 as broken or
plane changes. (Separate nuance: for *collision* resistance Grover does not even apply
directly; the BHT-style bounds are ~2^(b/3), and Bernstein argues even those are
pessimistic under real cost models.)

---

## 5. ICP-Brasil / ITI post-quantum posture

**Reachability:** www.gov.br/iti/pt-br and subpages were reachable on 2026-09-02 (JS-only
widgets, e.g. the Plone search and consultation listings, could not be driven from a
scripted fetch — flagged where relevant).

**Legal basis:** ICP-Brasil was created by Medida Provisória nº 2.200-2, de 24 de agosto de
2001 (confirmed inside the DOC-ICP-01 resolution text: "competências previstas no art. 4° da
Medida Provisória n° 2.200-2, de 24 de agosto de 2001").

**Do ITI/ICP-Brasil docs address post-quantum algorithms? — YES (normative), as of 2026.**

- **DOC-ICP-01.01 – "Padrões e Algoritmos Criptográficos da ICP-Brasil", version 6.0,
  approved by Instrução Normativa ITI nº 35, de 30 de janeiro de 2026**, adds **ML-DSA
  (FIPS 204)** and **ML-KEM**, and the new certificate types SE-S, SE-H, AE-S, AE-H
  (change-control entry, verified in the consolidated PDF on iti.gov.br).
  https://www.gov.br/iti/pt-br/assuntos/legislacao/documentos-principais (link:
  IN2022_22_DOC_ICP_01.01_compilada.pdf)
- Current algorithm mandates in DOC-ICP-01.01 v6.0 (extracted text):
  - AC (incl. Raiz) asymmetric key generation: RSA 4096 | brainpoolP512r1 | Ed448 | E-521 |
    **ML-DSA-(65 or 87)**.
  - End-user key generation (types A1/A2/A3/A CF-e-SAT/S1/S2/S3/T3/OM-BR/SE-S/SE-H/AE-S/AE-H):
    RSA 2048 | brainpoolP256r1 | Curve25519 | Ed25519 | Ed448 | E-521 | **ML-DSA-44**.
  - Key establishment: 3DES-112 / AES-128 / AES-256 | **ML-KEM-768 or 1024**.
  - TLS-style KX groups: ECDHE X25519 / X448, RSA 2048 / RSA 4096, **ML-KEM-512 / 768 / 1024**.
  - Signature suites: sha256-/sha512-WithRSAEncryption and WithECDSAEncryption, plus new
    **id-ml-dsa-44 / id-ml-dsa-65 / id-ml-dsa-87**.
  - History (change control): IN nº 08/2019 (v4.2) removed SHA-1 and RSA-1024 for end-user
    certs and RSA-2048 for AC certs; SHA-2 family is the hash baseline.
- **No post-quantum *roadmap or public consultation* was found.** The Consulta Pública area
  (assuntos/consulta-publica, consultas-atuais/anterioras) showed no PQ item, but those
  listings are JS-rendered, so this is "no evidence found", not an exhaustive negative. The
  site's JS-driven search could not be queried; flagged as unverified rather than asserted.
- **Document naming (do not echo "ADR"):** ICP-Brasil's normative ladder is: Resoluções do
  Comitê Gestor (CG), **DOC-ICP** documents (consolidated versions of the resolutions),
  and Instruções Normativas (IN) of ITI. **DOC-ICP-01** = "Declaração de Práticas de
  Certificação da Autoridade Certificadora Raiz da ICP-Brasil" (Resolução CG nº 192, de
  16/11/2021; consolidated v6.2). The certificate-policy requirements doc is **DOC-ICP-04**
  "Requisitos Mínimos para as Políticas de Certificado na ICP-Brasil" (Resolução CG nº 179,
  2020). The cryptographic-algorithm doc is **DOC-ICP-01.01** (above). No document type
  "ADR" exists in the ICP-Brasil corpus that I found.
  https://www.gov.br/iti/pt-br/assuntos/legislacao/documentos-principais

---

## 6. libp2p noise post-quantum

**Facts (read 2026-09-02, repo libp2p/rust-libp2p @ master):**

- `libp2p-noise` (master, v0.47.0) dependencies: `x25519-dalek 3`, `libp2p-identity` with
  `ed25519`, `snow 0.10` (ring-resolver off-wasm; default-resolver on wasm). No ML-KEM/noise
  PQ crate. → The Noise transport remains the classical **Noise_XX / X25519** handshake with
  identity-key signatures; **no post-quantum or hybrid option ships today**.
  https://raw.githubusercontent.com/libp2p/rust-libp2p/master/transports/noise/Cargo.toml
- PQ work in rust-libp2p (GitHub issue search, read 2026-09-02):
  - **TLS** (not noise): PR #6568 **merged 2026-07-31** — libp2p-tls moves from ring to
    rustls **aws-lc-rs** and advertises hybrid **X25519MLKEM768** (prefer-post-quantum),
    mirroring go-libp2p. So PQ key exchange exists on the *TLS* path since ~July 2026, not
    on the noise path. https://github.com/libp2p/rust-libp2p/pull/6568
  - **Noise**: PR #6481 (draft, open, not merged) proposes an off-by-default `mlkem-hfs`
    feature: hybrid `Noise_XXhfs_25519+ML-KEM-768_ChaChaPoly_SHA256` under a provisional
    protocol id `/noise-mlkem768-hfs/0.1.0`; notes it needs a cross-implementation spec and
    ML-KEM-768 in `snow`. https://github.com/libp2p/rust-libp2p/pull/6481
  - Issue #6462 (open): post-quantum (ML-DSA) peer identities. https://github.com/libp2p/rust-libp2p/issues/6462
- GlassChain relevance (repo evidence): pins libp2p **0.56.0** and libp2p-noise **0.46.1**
  (Cargo.lock) — both predate all of the above; noise is classical X25519.

---

## Unverifiable items (explicit)

1. FIPS 206 / FN-DSA: no CSRC record, no accessible draft PDF or DOI (all 404 on
   2026-09-02). Status cannot be confirmed from a primary source.
2. NIST IR 8547 final: only the November 2024 initial public draft was verifiable; the
   final/superseding version 404'd. Dates above are draft proposals.
3. ITI consultation listing contents (JS-rendered) and the site search could not be read
   directly; "no PQ consultation/roadmap found" is based on visible page content, not an
   exhaustive search.