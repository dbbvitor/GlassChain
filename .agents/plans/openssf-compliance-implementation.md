# OpenSSF Best Practices compliance — implementation plan

**Map:** [#123](https://github.com/dbbvitor/GlassChain) · **Date:** 2026-09-15 ·
**Decisions:** user accepted all recommended defaults (Q1–Q11 of the grill round).

## Goal

Implement the unblocked OpenSSF grilling tickets (#125–#128): approved
specifications + draft contents, leaving #129 (canonical compliance plan in
`docs/compliance/openssf-best-practices.md` + `.agents/plans/openssf-compliance.md`)
blocked until all four close.

## Shipped (2026-09-15, working tree — uncommitted)

- **Headers (#125):** `// SPDX-License-Identifier: Apache-2.0` + copyright on
  all 108 `crates/**/*.rs` (scripted, one pass). New `fuzz` target files too.
- **Governance (#125):** `GOVERNANCE.md` (roles, decisions, continuity,
  bus-factor honestly single), `CONTRIBUTING.md` (DCO via `git commit -s`,
  gates, style summary), `CODE_OF_CONDUCT.md` (Contributor Covenant v2.1
  verbatim, GitHub contact), `README.md` Roadmap section.
- **Security (#126):** `SECURITY.md` (PVR channel, 14-day response / 60-day
  fix SLA, GHSA credit, VEX = advisory analysis fields, pre-release support
  policy), `docs/threat-model.md` (assets, 5 trust boundaries, STRIDE-lite
  tables, accepted limitations, claims→evidence assurance map).
- **Quality (#127):** `codecov.yml` — ≥90% project coverage blocking status
  check. The plan-era note "badge reads 100%" was wrong (the gate passed
  vacuously: "no report found to compare against"); real measurement started
  2026-09-14 at 82.4%. Coverage work across the REPL/flows/RPC/core/identity/
  VM/network tests lifted it to **90.02% (2026-09-16)** and the gate is now
  pinned at `target: 90%` (+0.5% noise threshold); patch coverage stays
  informational. `fuzz.yml` — PR smoke (60s) + weekly deep (300s) runs of
  `fuzz-wire` and `fuzz-transactions`; harnesses in
  `crates/{core,network}/fuzz/` (standalone workspaces, not main-workspace
  members). `reproducible.yml` — weekly Linux-only build-twice hash compare,
  cross-runner determinism honestly out of scope.
- **Release (#128):** `release.yml` on `v*` tags — cargo-auditable build,
  CycloneDX SBOMs into `sbom/`, git-cliff CHANGELOG + notes, cosign keyless
  sign-blob, `gh release create` publishing binaries + signatures + SBOMs.
- **Doc sync:** AGENTS.md (CI paragraph + file-header convention bullet),
  README (navigation links, roadmap, threat-model row).

## Open items

- ~~**GitHub writes blocked**~~ **Done (2026-09-15):** `gh` re-authenticated —
  #125–#128 claimed, resolution comments posted, all four closed; map #123
  Decisions-so-far appended and resolved "Not yet specified" items pruned.
- ~~**Branch protection**~~ **Done (2026-09-15):** `main` protection enabled —
  required checks: Format, Clippy, Test ×3 OSes, Code coverage, DCO sign-off,
  Security audit, `codecov/project`; `strict=false`, admins not enforced.
- **`good first issue`/`small task` labels:** no current open issue
  qualifies (#74, #61 are deferred enhancements); labels will be applied as
  small tasks land — stated honestly in the #125 resolution comment.
- **#129:** synthesize the canonical plan once all four close.

## Verification

`cargo fmt --all --check` clean ·
`cargo check --workspace --all-targets --all-features --locked` clean ·
`cargo clippy ... -- -D warnings` clean ·
`cargo test ... --locked`: 626 passed / 0 failed ·
`cargo check` on both standalone fuzz workspaces clean.
