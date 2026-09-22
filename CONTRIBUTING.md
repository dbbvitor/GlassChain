# Contributing to GlassChain

Thank you for considering a contribution. GlassChain is a pre-release Rust
workspace (12 crates, toolchain pinned to 1.98.1) — the project favors clean,
direct evolution over backwards compatibility.

## Developer Certificate of Origin (DCO)

All code changes require a DCO assertion. Sign off every commit with:

```bash
git commit -s -m "feat: describe the change"
```

This adds a `Signed-off-by:` line certifying that you wrote the change or have
the right to submit it under the project license ([Apache-2.0](LICENSE)) —
see the [DCO text](https://developercertificate.org/) for the full statement.
CI verifies every commit in a PR carries a matching sign-off. Bot-authored
commits (`dependabot[bot]`, `github-actions[bot]`, …) are exempt, since a bot
cannot assert the DCO; the maintainer who merges the PR takes responsibility
for it.

## Getting started

```bash
git clone git@github.com:dbbvitor/GlassChain.git
cd GlassChain
make setup     # pinned toolchain + rustfmt/clippy + protoc
make test      # full suite via nextest (same flags as CI)
```

A first build pulls `wasmtime`, `libp2p`, and `tonic` — expect several minutes.

The blocking PR gate (`analysis.yml`: dependency hygiene, spelling, snarf,
feature matrix, cargo-careful, diff mutants) and the scheduled deep checks
(`deep-checks.yml`: miri, full mutants, ASan/LSan, Kani, Verus) are documented
in [ADR-019](docs/adr/adr-019-analysis-stack.md).

## Before opening a pull request

These are the same gates CI runs; finding a failure locally is cheaper:

```bash
make ci        # check (fmt + clippy + type-check) -> test -> analysis (deny + machete)
```

The raw cargo equivalents (useful when the Makefile's optional tooling is not
installed — `make tools` installs it):

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo nextest run --profile ci --workspace --lib --bins --tests --all-features --locked
```

Requirements:

1. **Sign-off** — every commit carries `Signed-off-by:` (DCO, above).
2. **Tests** — add or update tests for every behavior change, even small ones.
3. **Commits** — follow the existing Conventional Commits style (`feat:`,
   `fix:`, `refactor:`, or a short imperative sentence), subject under ~72
   characters.
4. **Documentation** — update `AGENTS.md`, `docs/operations.md`, or other docs
   whenever behavior, interfaces, or security controls change. Docs must never
   drift from working-tree code.
5. **No secrets, no build output** — never commit keys, certificates, `.pem`
   files, or `target/` artifacts.

## Code style

Most style is enforced mechanically:

- `unsafe` code is denied workspace-wide; there is zero `unsafe` today.
- Clippy runs `all` + `pedantic` + `nursery` + `cargo`; warnings are errors in
  CI. Use targeted `#[allow(clippy::...)]` with a one-line justification.
- Errors are `thiserror` enums per crate (`error.rs`); propagate with `?`.
- Prices are integers in minor currency units (`1500` = $15.00). Never floats.
- Format only files you touched: `cargo fmt -- crates/.../file.rs` (a full
  `cargo fmt --all` is reserved for its own change).

See `AGENTS.md` for the complete list of conventions, crate layout, and
security invariants.

## Reporting issues

- Bugs and enhancements: [GitHub Issues](https://github.com/dbbvitor/GlassChain/issues).
- **Security vulnerabilities: do not open a public issue.** Use
  [GitHub Private Vulnerability Reporting](https://github.com/dbbvitor/GlassChain/security/advisories/new)
  — see [SECURITY.md](SECURITY.md).
- Issues labeled `good first issue` or `small task` are curated entry points.

## Where things live

- [README.md](README.md) — overview and quick start
- [docs/architecture.md](docs/architecture.md) and [docs/adr/](docs/adr/) —
  architecture and accepted decisions
- [PLUGIN_KIT.md](PLUGIN_KIT.md) — provider traits, read before extending one
- [docs/operations.md](docs/operations.md) — CLI/protocol/API reference
