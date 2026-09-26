# Dependency cluster constraints

## BLS12-381 algebraic stack: `group`/`pairing` are pinned by `blstrs`

The BFT code and the `bft_verify` bench sit on two zkcrypto ecosystems that
move together and cannot be mixed:

- **production**: `bls-signatures 0.15.0` → `blstrs 0.7.1` → `ff 0.13.1`,
  `group 0.13.0`, `pairing 0.23.0` (blstrs implements pairing's traits with a
  blst-backed curve);
- **dev-only**: `bls12_381 0.9.0` (the superseded pure-Rust comparison path)
  → `ff 0.14`, `group 0.14`.

Both `blstrs 0.7.1` and `bls-signatures 0.15.0` are the latest releases and
still require `group ^0.13` / `pairing ^0.23`, so `group 0.14` / `pairing
0.24` cannot be installed without duplicating the production graph. Evidence
(2026-09-26):

- `cargo outdated --workspace`: `group 0.13.0 → 0.14.0` and
  `pairing 0.23.0 → 0.24.0` have no `Compat` entry.
- `cargo upgrade --incompatible --dry-run` reports exactly those two.
- crates.io dependency metadata for blstrs 0.7.1 / bls-signatures 0.15.0:
  `group ^0.13`, `pairing ^0.23`.
- The closed bumps failed: PR #138 (`group 0.14`) and PR #177 (`pairing
  0.24`) both tripped Dependency hygiene (duplicate ff/group/pairing;
  `deny.toml` bans duplicates in the production graph) and Clippy
  (`blstrs::Bls12` no longer resolves `multi_miller_loop` against pairing
  0.24).

Notes:

- `cargo-deny` bans duplicates in the production graph only, which is why the
  dev-only `bls12_381 0.9` duplicates (`ff`/`group` 0.14) pass. The
  `bft_verify` bench mixes both ecosystems on purpose.
- `pairing` is not named in production code at all; the `bft` feature no
  longer pulls it (only the bench does, as a dev-dependency).
- `.github/dependabot.yml` ignores the known-blocked majors. Revisit when a
  `blstrs`/`bls-signatures` release moves to the 0.14/0.24 line: bump
  `group`/`pairing` in lockstep, lift the ignores, and re-run `cargo deny`.
