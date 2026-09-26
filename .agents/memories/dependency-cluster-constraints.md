# Dependency cluster constraints

## BLS12-381 algebraic stack: one generation at a time

The BFT code and the `bft_verify` bench sit on one zkcrypto algebraic
generation: `bls-signatures 0.15.0` → `blstrs 0.7.1` → `ff 0.13.1`,
`group 0.13.0`, `pairing 0.23.0` (blstrs implements pairing's traits with a
blst-backed curve). The bench's superseded pure-Rust comparison path is pinned
to `bls12_381 0.8`, which shares that generation.

Both `blstrs 0.7.1` and `bls-signatures 0.15.0` are the latest releases and
still require `group ^0.13` / `pairing ^0.23`, so `group 0.14` / `pairing
0.24` / `bls12_381 0.9` (which pulls ff/group 0.14) cannot be installed
without duplicating the production stack. Evidence (2026-09-26):

- `cargo outdated --workspace`: `group 0.13.0 → 0.14.0` and
  `pairing 0.23.0 → 0.24.0` have no `Compat` entry.
- `cargo upgrade --incompatible --dry-run` reports exactly those two.
- crates.io dependency metadata for blstrs 0.7.1 / bls-signatures 0.15.0:
  `group ^0.13`, `pairing ^0.23`.
- Closed bumps: PR #138 (`group 0.14`) and PR #177 (`pairing 0.24`) failed
  Dependency hygiene (duplicates; `deny.toml` bans them) and Clippy
  (`blstrs::Bls12` no longer resolves `multi_miller_loop`).

Dependabot still proposes these bumps — correct, since a future
blstrs/bls-signatures release makes them valid. Evaluate each PR; close it
with the evidence until the upstream move lands, then bump `group`, `pairing`
and `bls12_381` together in lockstep.

## Duplicate-version ratchet

Duplicate crate *generations* are tolerated only when listed in the two
allowlists, and each list must contain exactly what the tool reports:

- `deny.toml` `[bans] skip` — the **production** graph only (19 names as of
  2026-09-26; cargo-deny ignores dev-only duplicates). Verify with
  `cargo deny --all-features check bans`.
- `clippy.toml` `allowed-duplicate-crates` — all targets, including dev (25
  names). Verify with `cargo clippy --workspace --all-targets --all-features`.

The dedupe pass (PR #188 follow-up) removed six duplicate names and pinned the
reachable ones:

- `bls12_381 0.8` (dev) — shares ff/group/pairing 0.13/0.13/0.23 with blstrs;
  removed the `ff`, `group` and `pairing` duplicates.
- `wat ~1.254` (dev, all four declaring crates) — shares `wast 254` /
  `wasm-encoder 0.254` / `wasmparser 0.254` with wasmtime 48; removed the
  `wasm-encoder` and `wasmparser` duplicates.
- `yoke-derive 0.8.2` + `zerofrom-derive 0.1.7` (lockfile `--precise` pins) —
  both use syn 2 / synstructure 0.13; removed the `synstructure` duplicate.
  (`syn 2/3` itself remains: async-trait, clap_derive, displaydoc,
  futures-macro and libp2p-swarm-derive are on syn 3.)

Remaining duplicates are upstream-locked and listed in the allowlists:
RustCrypto 0.9/0.10/0.11 generation splits (`digest`, `sha2`, `block-buffer`,
`crypto-common`, `cpufeatures`, `chacha20`, `curve25519-dalek`, `fiat-crypto`),
`rand`/`rand_core`/`getrandom` generations (bls-signatures 0.6, snow, ring),
`hashbrown`/`foldhash` (wasmtime, petgraph, gimli), `itertools` (criterion vs
prost/wasmtime), `syn`/`thiserror`/`bitflags`/`windows-sys`/`r-efi`/
`embedded-io`, and `rcgen`/`pem`/`yasna` (libp2p-tls 0.7, the latest, still
pins `rcgen ^0.13`).

Because the two lists are pruned to exactly what the tools report, a plain
`cargo update` that reintroduces a pinned duplicate fails clippy/deny and must
re-pin `--precise` deliberately.
