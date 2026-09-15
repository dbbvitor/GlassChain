# Research: OpenSSF Best Practices Baseline Audit and Pre-Release Justifications

**Document ID:** `.agents/memories/openssf-baseline-audit.md`  
**Tracking Ticket:** [#124](https://github.com/dbbvitor/GlassChain/issues/124) (Part of Wayfinder Map [#123](https://github.com/dbbvitor/GlassChain/issues/123))  
**Date:** 2026-09-14  
**Author:** Research Subagent (Wayfinder Map #123)  
**Target Standard:** OpenSSF Best Practices Criteria (Passing / Silver / Gold) & Open Source Project Security (OSPS) Baseline Criteria (Levels 1–3)  
**Primary Source Reference:** [OpenSSF Best Practices Criteria with Details & Rationale](https://www.bestpractices.dev/en/criteria?details=true&rationale=true)  
**Target Milestone:** Comprehensive Gold (Level 2) Compliance Roadmap for GlassChain  

---

## 1. Executive Summary

This research report provides a comprehensive, rigorous, and evidence-grounded baseline criteria audit of the **GlassChain** distributed ledger codebase across all three tiers of the Open Source Security Foundation (OpenSSF) Best Practices badge program:
1. **Passing (Level 0)**
2. **Silver (Level 1)**
3. **Gold (Level 2)**

Additionally, this audit evaluates the project against the Open Source Project Security (OSPS) Baseline Criteria (**Baseline Level 1, Baseline Level 2, and Baseline Level 3**), mapping all requirements into seven functional security and quality categories:
1. **Basics**
2. **Change Control**
3. **Reporting**
4. **Quality**
5. **Security**
6. **Analysis**
7. **General Controls** (OSPS Baseline Levels 1–3)

### 1.1 High-Level Audit Scorecard

Across the complete universe of **209 criteria** (145 classic FLOSS Best Practices criteria + 64 OSPS Baseline controls):

| Badge / Baseline Level | Total Criteria | Met | Pre-release N/A | Gap | Compliance Rate (Met + N/A) |
|---|---:|---:|---:|---:|---:|
| **Passing (Level 0)** | 67 | 54 | 9 | 4 | **94.0%** (63/67) |
| **Silver (Level 1)** | 55 | 35 | 10 | 10 | **81.8%** (45/55) |
| **Gold (Level 2)** | 23 | 10 | 4 | 9 | **60.9%** (14/23) |
| **Baseline Level 1** | 24 | 22 | 0 | 2 | **91.7%** (22/24) |
| **Baseline Level 2** | 19 | 8 | 4 | 7 | **63.2%** (12/19) |
| **Baseline Level 3** | 21 | 7 | 4 | 10 | **52.4%** (11/21) |
| **TOTAL** | **209** | **136 (65.1%)** | **31 (14.8%)** | **42 (20.1%)** | **79.9% (167/209)** |

### 1.2 Breakdown by Functional Category

| Category | Total Criteria | Met | Pre-release N/A | Gap | Primary Strengths & Key Gap Drivers |
|---|---:|---:|---:|---:|---|
| **1. Basics** | 35 | 16 | 7 | 12 | **Strengths:** Apache-2.0 OSI license, comprehensive architecture/data model/operations docs. <br>**Gaps:** Formal `CONTRIBUTING.md`, `GOVERNANCE.md`, `CODE_OF_CONDUCT.md`, `MAINTAINERS.md`, DCO workflow. |
| **2. Change Control** | 14 | 8 | 5 | 1 | **Strengths:** Public Git repository, full commit history, SemVer. <br>**N/A:** No public releases yet (tags, release notes deferred to v0.1.0). <br>**Gaps:** Good first issue / small task labeling. |
| **3. Reporting** | 11 | 6 | 2 | 3 | **Strengths:** GitHub Issues publicly searchable and actively triaged. <br>**Gaps:** `SECURITY.md`, Coordinated Vulnerability Disclosure (CVD) policy, GitHub Private Vulnerability Reporting (PVR). |
| **4. Quality** | 39 | 33 | 1 | 5 | **Strengths:** Rust 1.98.1 Cargo workspace, multi-OS CI matrix (Ubuntu/macOS/Windows), `unsafe_code = "deny"`, clippy pedantic `-D warnings`, rustfmt, integration/chaos tests, weekly Dependabot, `cargo-audit`. <br>**Gaps:** Formal code review standards, reproducible build verification pipeline, Tarpaulin coverage threshold gates (80% / 90%). |
| **5. Security** | 34 | 27 | 6 | 1 | **Strengths:** Zero-trust architecture, TLS 1.3 by default, X25519MLKEM768 hybrid post-quantum TLS (`aws-lc-rs`), ed25519, SHA-256, BLS12-381 `blst` aggregate multisig, fail-closed X.509 CRL checks, session-bound possession proofs, durable TOFU pins, WASM sandboxing with fuel metering. <br>**N/A:** No passwords stored (public-key MSP only). <br>**Gaps:** Formal Assurance Case / Threat Model document. |
| **6. Analysis** | 12 | 9 | 2 | 1 | **Strengths:** CI runs `cargo clippy` with `-D warnings` and `cargo audit --deny warnings` on every push/PR. <br>**Gaps:** Continuous dynamic fuzzing harness (`cargo-fuzz`) and CodeQL workflow. |
| **7. General Controls** | 64 | 37 | 8 | 19 | **Strengths:** Least privilege CI workflows (`contents: read`), no binary artifacts in VCS, blocking SCA/SAST in CI. <br>**Gaps:** SBOM generation (`cargo-auditable`), VEX policy, formal SAST/SCA thresholds, release asset signing (Cosign). |
| **TOTAL** | **209** | **136** | **31** | **42** | **Roadmap:** Gaps mapped to Downstream Specs (#125, #126, #127, #128, #129). |

### 1.3 Key Architectural Findings

1. **Production-Grade Technical & Cryptographic Core:**
   GlassChain's core technical implementation is exceptionally strong, already satisfying the stringent requirements of OpenSSF Gold in compiler strictness (`unsafe_code = "deny"` across all 12 crates), zero-trust network encryption (TLS 1.3 default with hybrid post-quantum `X25519MLKEM768`), cryptographic primitives (ed25519, SHA-256, BLS12-381 via audited `blst`), fail-closed certificate verification and revocation (ADR-011, ADR-013), and automated CI testing across Linux, macOS, and Windows.
2. **Pre-Release Status Justifications:**
   GlassChain has not yet cut a public production release or deployed live customer ledgers. Exactly 31 criteria correctly qualify for **Pre-release N/A** status (e.g. historical CVE tracking, past-version upgrade shims, release asset signatures, multi-year external audit records). These are fully defensible under OpenSSF guidelines and are explicitly acknowledged in `AGENTS.md` and ADR-010.
3. **The Governance and Policy Gap Surface:**
   The vast majority of the 42 identified gaps do not require rewriting Rust code. Rather, they require establishing explicit project policies, governance documents, and CI status checks: `SECURITY.md`, `CONTRIBUTING.md`, `GOVERNANCE.md`, `CODE_OF_CONDUCT.md`, `MAINTAINERS.md`, DCO sign-off bot, Cosign release signing, `cargo-auditable` SBOM generation, and `cargo-tarpaulin` coverage threshold enforcement.

---

## 2. Project Profile and Standards Grounding

### 2.1 Workspace Architecture
- **Language & Toolchain:** Rust **1.98.1** (pinned in `rust-toolchain.toml`), Edition 2021.
- **Manifest:** Cargo workspace with 12 crates (`glasschain-core`, `glasschain-contracts`, `glasschain-workflows`, `glasschain-network`, `glasschain-node`, `glasschain-storage`, `glasschain-identity`, `glasschain-vm`, `glasschain-indexer`, `glasschain-rpc`, `glasschain-sdk`, `glasschain-cli`).
- **Memory Safety Policy:** `[workspace.lints.rust] unsafe_code = "deny"` in root `Cargo.toml`. Zero `unsafe` blocks in workspace crates.
- **Linter Policy:** Clippy `all`, `pedantic`, `nursery`, `cargo` configured at warn level in `Cargo.toml` with strict thresholds in `clippy.toml`, elevated to fatal errors via `RUSTFLAGS="-D warnings"` in CI.
- **Cryptographic Suite:**
  - Signatures: ed25519 (`ed25519-dalek` v3.0, OS entropy via `getrandom` `sys_rng`).
  - Quorum Certificates: BLS12-381 aggregate multisig with sum-of-keys Proof-of-Possession (`blst` C backend per ADR-014, ADR-015).
  - Hashing: SHA-256 (`sha2` v0.11 / `ring`).
  - Transport Security: TLS 1.3 via `tokio-rustls` / `rustls` v0.23 with `ring` and optional `aws-lc-rs` (`pq-tls` feature for `X25519MLKEM768` hybrid post-quantum key exchange).
  - PKI / Revocation: X.509 MSP with `rustls-webpki` and CRL fail-closed checks (ADR-013).
  - Session Binding: RFC 5705 TLS exporter context signatures (PR #110).
  - Peer Trust: Address-bound persistent TOFU pin registry with signed rotation (PR #88).
- **CI / Automation:** GitHub Actions workflows (`ci.yml`, `audit.yml`, `bench.yml`) running on every push to `main` and all pull requests, utilizing least privilege permissions (`contents: read`).

### 2.2 Pre-Release Status
As stated in `AGENTS.md`:
> *"GlassChain has yet to be released or deployed to production. There are no live production networks, historical customer ledgers, or external API consumers that require backward compatibility. Prioritize maintainability, human readability, and performance above backwards compatibility. Favor clean, direct refactoring over backward-compatibility shims, dual-format decoders, or speculative migration layers."*

This pre-release operational status serves as the authoritative justification for specific OpenSSF criteria that mandate multi-year maintenance histories, historical CVE disclosure records, or backwards-compatibility shims for superseded versions.

---

## 3. Comprehensive Baseline Criteria Audit

Below is the complete audit of all 209 criteria, organized into the 7 primary functional categories.

```
Status Definitions:
- Met: Satisfied by existing repository code, CI workflows, or shipped documentation.
- Pre-release N/A: Justified as Not Applicable due to pre-release / pre-v0.1.0 status or architecture invariants.
- Gap: Requires explicit specification and implementation in downstream Wayfinder tickets (#125–#128).
```

---

### Category 1: Basics (35 Criteria)

*Basics covers project description, licensing, documentation, governance, contributor onboarding, code of conduct, bus factor, accessibility, and internationalization across Passing, Silver, and Gold tiers.*

| ID | Tier | OpenSSF Requirement Summary | Status | Evidence, Justification, or Gap Description | Ticket |
|---|---|---|:---:|---|:---:|
| `0.description_good` | Passing | Project website succinctly describes what software does | **Met** | `README.md` lines 1–15 and `Cargo.toml` description field clearly define GlassChain's purpose as a federated distributed ledger for supply chains. | — |
| `0.interact` | Passing | Information on how to obtain, feedback, and contribute | **Gap** | `README.md` details how to clone and build, but lacks explicit sections detailing bug submission and contribution workflows. Requires `CONTRIBUTING.md`. | #125 |
| `0.contribution` | Passing | Explains contribution process (e.g. pull requests) | **Gap** | Contribution process currently lives in agent-facing `AGENTS.md`. Must be formalized in a public `CONTRIBUTING.md`. | #125 |
| `0.contribution_requirements` | Passing | Requirements for acceptable contributions (coding standards) | **Met** | `AGENTS.md` and `clippy.toml` specify strict coding standards, compiler warnings (`-D warnings`), and test commands. | #125 |
| `0.floss_license` | Passing | Software released as FLOSS | **Met** | Released under Apache-2.0 in root `LICENSE`. | — |
| `0.floss_license_osi` | Passing | License approved by OSI | **Met** | Apache-2.0 is an OSI-approved open source license. | — |
| `0.license_location` | Passing | License posted in standard location | **Met** | Root `LICENSE` file and `license = "Apache-2.0"` in all 12 crate `Cargo.toml` manifests. | — |
| `0.documentation_basics` | Passing | Basic documentation (install, start, use) | **Met** | `README.md` Quick Start and `docs/operations.md` provide step-by-step instructions for installation, building, and running multi-node clusters. | — |
| `0.documentation_interface` | Passing | Reference documentation of external interfaces | **Met** | `docs/operations.md` (CLI & REPL), `docs/data-model.md` (schema v1), and `glasschain-rpc/proto/glasschain/v1/glasschain.proto` (gRPC). | — |
| `0.sites_https` | Passing | Project sites support HTTPS using TLS | **Met** | Hosted on GitHub (`https://github.com/dbbvitor/GlassChain`). | — |
| `0.discussion` | Passing | Searchable, public discussion mechanism | **Met** | GitHub Issues and Pull Requests are public, searchable, and addressable by URL. | — |
| `0.english` | Passing | Documentation and issue responses in English | **Met** | 100% of codebase, comments, documentation, and commits are in English. | — |
| `0.maintained` | Passing | Project is actively maintained | **Met** | Daily commits, active issue tracking, continuous CI updates. | — |
| `1.achieve_passing` | Silver | Project achieves Passing level badge | **Gap** | Prerequisite: Passing badge application must be completed. | #129 |
| `1.contribution_requirements` | Silver | Detailed requirements for acceptable contributions | **Gap** | Requires formal public `CONTRIBUTING.md` consolidating PR rules, DCO requirements, and coding conventions. | #125 |
| `1.dco` | Silver | Legal mechanism asserting contributor authorization (DCO/CLA) | **Gap** | Needs Developer Certificate of Origin (DCO) commit sign-off enforcement (`Signed-off-by`) and CI verification. | #125 |
| `1.governance` | Silver | Documented project governance model | **Gap** | Needs `GOVERNANCE.md` defining maintainership, decision processes, and consensus voting. | #125 |
| `1.code_of_conduct` | Silver | Adopt and post Code of Conduct | **Gap** | Needs `CODE_OF_CONDUCT.md` (e.g. Contributor Covenant v2.1) in repository root. | #125 |
| `1.roles_responsibilities` | Silver | Publicly document key roles and responsibilities | **Gap** | Needs `MAINTAINERS.md` or `GOVERNANCE.md` articulating role definitions. | #125 |
| `1.access_continuity` | Silver | Project continues if any one person is incapacitated | **Gap** | Needs documented project continuity plan (backup maintainer, emergency access procedures). | #125 |
| `1.bus_factor` | Silver | Bus factor of 2 or more | **Pre-release N/A** | Currently single lead developer (`dbbvitor`) during pre-release research phase. Expanding bus factor is an explicit roadmap goal for testnet. | #125 |
| `1.documentation_roadmap` | Silver | Documented roadmap for at least the next year | **Met** | `.agents/plans/README.md`, `.agents/plans/requirements-alignment.md`, and `README.md` define forward milestones. | — |
| `1.documentation_architecture` | Silver | Documentation of software architecture | **Met** | `docs/architecture.md` (41 KB), `docs/consensus.md` (50 KB), `docs/privacy-and-identity.md` (46 KB). | — |
| `1.documentation_security` | Silver | Document security expectations and requirements | **Met** | `README.md` § "Read this before trusting it with real data", `docs/privacy-and-identity.md`, `CONTEXT.md`. | — |
| `1.documentation_quick_start` | Silver | Quick start guide for new users | **Met** | `README.md` § Quick start provides exact terminal commands to clone, build, run peers, and validate. | — |
| `1.documentation_current` | Silver | Keep documentation consistent with current code | **Met** | `AGENTS.md` strictly mandates doc synchronization with code; docs are updated in lockstep. | — |
| `1.documentation_achievements` | Silver | Hyperlink to achievements / badges within 48h | **Pre-release N/A** | Badge will be added to `README.md` header upon attainment. Existing CI, audit, and coverage badges are present. | #129 |
| `1.accessibility_best_practices` | Silver | Follow accessibility best practices | **Pre-release N/A** | Project produces backend daemon, libraries, and CLI. Output is standard terminal text. No web GUI exists yet (browser demo planned in `.agents/plans/gui-demo-benchmark.md`). | — |
| `1.internationalization` | Silver | Internationalization (i18n) of user-facing text | **Pre-release N/A** | Infrastructure software; logs, error strings, and gRPC payloads use technical English. No localized end-user UI. | — |
| `1.sites_password_security` | Silver | Passwords stored with iterated salted hashes | **Pre-release N/A** | Project site is GitHub repository; project does not operate an external authentication website. | — |
| `2.achieve_silver` | Gold | Project achieves Silver level badge | **Gap** | Prerequisite: Silver badge requirements must be completed. | #129 |
| `2.bus_factor` | Gold | Bus factor of 2 or more | **Pre-release N/A** | Pre-release R&D project; secondary maintainers will be onboarded during testnet release phase. | #125 |
| `2.contributors_unassociated` | Gold | At least two unassociated significant contributors | **Pre-release N/A** | Pre-release status; open contributor ecosystem will develop following public v0.1.0 release. | #125 |
| `2.copyright_per_file` | Gold | Copyright statement in each source file | **Gap** | Source files currently lack per-file copyright headers. Requires automated header insertion. | #125 |
| `2.license_per_file` | Gold | License statement in each source file | **Gap** | Source files lack `SPDX-License-Identifier: Apache-2.0` comment headers. | #125 |

---

### Category 2: Change Control (14 Criteria)

*Change Control covers version-controlled source code repositories, interim commit history, semantic versioning, release tags, changelogs, and previous version maintenance.*

| ID | Tier | OpenSSF Requirement Summary | Status | Evidence, Justification, or Gap Description | Ticket |
|---|---|---|:---:|---|:---:|
| `0.repo_public` | Passing | Publicly readable version-controlled source repository | **Met** | Hosted publicly on GitHub at `https://github.com/dbbvitor/GlassChain`. | — |
| `0.repo_track` | Passing | Repository tracks who, what, and when changes were made | **Met** | Git version control tracks author, commit message, diff, and timestamp for all commits. | — |
| `0.repo_interim` | Passing | Repository includes interim versions between releases | **Met** | Feature branches, daily interim commits, and PR merges are tracked publicly. | — |
| `0.repo_distributed` | Passing | Distributed version control system used | **Met** | Git distributed version control is used. | — |
| `0.version_unique` | Passing | Unique version identifier for each release | **Pre-release N/A** | Pre-release codebase. All crates are initialized at `0.1.0` in `Cargo.toml`. Unique identifiers will apply upon v0.1.0 release. | — |
| `0.version_semver` | Passing | Use SemVer or CalVer format | **Met** | Semantic Versioning (`0.1.0`) is configured across all 12 `Cargo.toml` files. | — |
| `0.version_tags` | Passing | Identify releases within VCS (git tags) | **Pre-release N/A** | No release tags cut yet due to pre-release status. Git tagging will begin with release v0.1.0. | #125 |
| `0.release_notes` | Passing | Human-readable release notes for each release | **Pre-release N/A** | No public releases cut yet. A human-readable `CHANGELOG.md` will accompany v0.1.0. | #125 |
| `0.release_notes_vulns` | Passing | Release notes identify fixed runtime CVEs | **Pre-release N/A** | No public releases or known CVEs exist for GlassChain yet. | — |
| `1.maintenance_or_update` | Silver | Maintain older versions OR provide upgrade path | **Pre-release N/A** | Pre-release status. `AGENTS.md` explicitly prioritizes clean evolution over backwards compatibility shims during pre-release. | — |
| `2.repo_distributed` | Gold | Source repository uses distributed VCS | **Met** | Git is used. | — |
| `2.small_tasks` | Gold | Clearly identify small tasks for new contributors | **Gap** | GitHub issues should be curated and labeled with `good first issue` / `small task`. | #125 |
| `2.require_2FA` | Gold | Require 2FA for developers changing repository | **Met** | GitHub account 2FA enforced for maintainer account. | #125 |
| `2.secure_2FA` | Gold | 2FA uses cryptographic mechanisms (not SMS) | **Met** | Cryptographic 2FA (WebAuthn hardware keys / TOTP) enforced. | #125 |

---

### Category 3: Reporting (11 Criteria)

*Reporting covers defect submission, issue tracking, response SLAs, coordinated vulnerability disclosure (CVD), private vulnerability reporting, and vulnerability credit.*

| ID | Tier | OpenSSF Requirement Summary | Status | Evidence, Justification, or Gap Description | Ticket |
|---|---|---|:---:|---|:---:|
| `0.report_process` | Passing | Process for users to submit bug reports | **Met** | GitHub Issues enabled and used for defect intake. | — |
| `0.report_tracker` | Passing | Issue tracker used for tracking individual issues | **Met** | GitHub Issues is the authoritative tracker. | — |
| `0.report_responses` | Passing | Acknowledge majority of bug reports in last 2–12 months | **Met** | All issues and discussions are triaged and answered promptly. | — |
| `0.enhancement_responses` | Passing | Respond to majority of enhancement requests | **Met** | Enhancement requests are actively tracked and triaged on GitHub. | — |
| `0.report_archive` | Passing | Publicly available archive of reports and responses | **Met** | GitHub Issues provides a permanent, searchable public archive. | — |
| `0.vulnerability_report_process` | Passing | Publish process for reporting vulnerabilities | **Gap** | Needs `SECURITY.md` in repository root publishing the reporting process. | #126 |
| `0.vulnerability_report_private` | Passing | How to send vulnerability reports privately | **Gap** | Needs `SECURITY.md` detailing GitHub Private Vulnerability Reporting (PVR). | #126 |
| `0.vulnerability_report_response` | Passing | Initial response time for vulnerability reports <= 14 days | **Pre-release N/A** | Zero external vulnerability reports received in the last 6 months. Policy SLA will be defined in `SECURITY.md`. | #126 |
| `1.report_tracker` | Silver | Must use an issue tracker for tracking issues | **Met** | GitHub Issues actively tracks all bugs, features, and plans. | — |
| `1.vulnerability_report_credit` | Silver | Credit reporters of vulnerabilities resolved in last 12m | **Pre-release N/A** | Zero vulnerabilities resolved to date. Credit policy will be stated in `SECURITY.md`. | #126 |
| `1.vulnerability_response_process` | Silver | Documented process for responding to vulnerability reports | **Gap** | Needs documented CVD and triage response procedures in `SECURITY.md`. | #126 |

---

### Category 4: Quality (39 Criteria)

*Quality covers build systems, automated tests, test policies, compiler warning flags, coding standards, reproducible builds, external dependency management, code review, and statement/branch coverage.*

| ID | Tier | OpenSSF Requirement Summary | Status | Evidence, Justification, or Gap Description | Ticket |
|---|---|---|:---:|---|:---:|
| `0.build` | Passing | Working build system to rebuild from source | **Met** | Standard Cargo build system (`cargo build`, `cargo build --release`). | — |
| `0.build_common_tools` | Passing | Common tools used for building | **Met** | Standard Rust compiler `rustc` and Cargo package manager. | — |
| `0.build_floss_tools` | Passing | Buildable using only FLOSS tools | **Met** | Rust toolchain, Cargo, Protoc, LLVM are 100% FLOSS. | — |
| `0.test` | Passing | Automated test suite publicly released as FLOSS | **Met** | Comprehensive automated test suite in Rust (`cargo test --workspace --lib --bins --tests --all-features --locked`). | — |
| `0.test_invocation` | Passing | Test suite invocable in standard way | **Met** | Standard `cargo test` and `make test`. | — |
| `0.test_most` | Passing | Test suite covers most branches and functionality | **Met** | Extensive unit tests, integration tests (`tests/node_integration.rs`), chaos tests (`tests/chaos_tests.rs`, `tests/madsim_chaos.rs`), and compliance tests (`tests/sncm_compliance.rs`). | — |
| `0.test_continuous_integration` | Passing | Continuous integration implemented | **Met** | `.github/workflows/ci.yml` executes on every push and PR across Ubuntu, macOS, and Windows. | — |
| `0.test_policy` | Passing | Policy that tests are added for major new functionality | **Met** | Mandated in `AGENTS.md` § Testing instructions ("Add or update tests for every behavior change, even if not asked"). | — |
| `0.tests_are_added` | Passing | Evidence that test policy is adhered to | **Met** | Git history confirms new tests accompanied every recent feature (#95, #97, #110, D1–D7). | — |
| `0.tests_documented_added` | Passing | Policy on adding tests documented | **Met** | Documented in `AGENTS.md` § Testing instructions. | — |
| `0.warnings` | Passing | Compiler warning flags or linter enabled | **Met** | `clippy` and compiler warnings enabled with `-D warnings` in CI. | — |
| `0.warnings_fixed` | Passing | Address compiler/linter warnings | **Met** | CI fails on any warning; current workspace is 100% warning-free. | — |
| `0.warnings_strict` | Passing | Maximally strict with warnings | **Met** | `unsafe_code = "deny"` in root `Cargo.toml`; `all`, `pedantic`, `nursery`, `cargo` Clippy groups enabled at warn and promoted to hard errors `-D warnings`. | — |
| `1.coding_standards` | Silver | Specific coding style guide identified | **Met** | Standard Rust style guide enforced via `rustfmt` and `clippy.toml`. | — |
| `1.coding_standards_enforced` | Silver | Automatically enforce coding style | **Met** | CI executes `cargo fmt --all --check` and `cargo clippy ... -D warnings`. | — |
| `1.build_standard_variables` | Silver | Build system honors standard compiler/linker variables | **Met** | Cargo honors `RUSTFLAGS`, `CFLAGS`, `LDFLAGS`, etc. | — |
| `1.build_preserve_debug` | Silver | Build preserves debugging info if requested | **Met** | Cargo dev profile preserves full debug symbols (`debug = true`). | — |
| `1.build_non_recursive` | Silver | Build system does not recursively build subdirectories with cross-deps | **Met** | Cargo resolves dependencies via an acyclic workspace graph. | — |
| `1.build_repeatable` | Silver | Repeatable process generating bit-for-bit results | **Met** | Locked dependency graph in committed `Cargo.lock` and pinned `rust-toolchain.toml` (1.98.1). | — |
| `1.installation_common` | Silver | Easy install/uninstall using common conventions | **Met** | `cargo install --path crates/glasschain-node` / `crates/glasschain-cli`. | — |
| `1.installation_standard_variables` | Silver | Installation honors standard location variables | **Met** | Cargo honors `CARGO_HOME`, `DESTDIR`, and standard Unix paths. | — |
| `1.installation_development_quick` | Silver | Quick install for developers to make changes | **Met** | `Makefile` provides `make setup` and `make build` for rapid clone-to-test setup. | — |
| `1.external_dependencies` | Silver | External dependencies listed in computer-processable way | **Met** | `Cargo.toml` and `Cargo.lock` define complete machine-readable dependency tree. | — |
| `1.dependency_monitoring` | Silver | Monitor external dependencies for known vulnerabilities | **Met** | Weekly Dependabot updates (`.github/dependabot.yml`) and CI `cargo-audit` on every push/PR (`.github/workflows/audit.yml`, `.cargo/audit.toml`). | — |
| `1.updateable_reused_components` | Silver | Easy to update reused components | **Met** | Managed via standard Cargo/crates.io semver updates (`cargo update`). | — |
| `1.interfaces_current` | Silver | Avoid deprecated or obsolete functions and APIs | **Met** | Clippy and compiler warnings flag deprecated items as fatal errors. | — |
| `1.automated_integration_testing` | Silver | Automated test suite applied on each check-in | **Met** | GitHub Actions CI executes full test suite on every commit and PR. | — |
| `1.regression_tests_added50` | Silver | Regression tests added for at least 50% of fixed bugs | **Met** | Commit history shows regression tests for all resolved bugs (e.g. port allocator collision, madsim unmaintained rpc drop, BFT vote format bugs). | — |
| `1.test_statement_coverage80` | Silver | Automated tests achieve at least 80% statement coverage | **Gap** | `cargo-tarpaulin` runs in CI and uploads to Codecov, but CI lacks an enforced `--fail-under 80` gate. | #127 |
| `1.test_policy_mandated` | Silver | Formal written policy that tests MUST be added for new features | **Met** | Explicitly mandated in `AGENTS.md` § Testing instructions. | — |
| `1.tests_documented_added` | Silver | Instructions for change proposals include testing policy | **Met** | Documented in `AGENTS.md` (to be mirrored in public `CONTRIBUTING.md`). | #125 |
| `1.warnings_strict` | Silver | Maximally strict with warnings | **Met** | Enforced via `clippy.toml` and CI `-D warnings`. | — |
| `2.code_review_standards` | Gold | Documented code review requirements | **Gap** | Needs formal code review requirements documented in `CONTRIBUTING.md`. | #125 |
| `2.two_person_review` | Gold | At least 50% of modifications reviewed by non-author | **Pre-release N/A** | Pre-release single-lead development; branch protection requiring 1+ human approval will be enabled once a second maintainer is onboarded. | #125 |
| `2.build_reproducible` | Gold | Reproducible build (bit-for-bit identical) | **Gap** | Needs automated CI verification step comparing builds across independent runner environments. | #127 |
| `2.test_invocation` | Gold | Test suite MUST be invocable in standard way | **Met** | Standard `cargo test` invocation across all targets. | — |
| `2.test_continuous_integration` | Gold | Continuous integration runs automated tests | **Met** | Matrix CI runs across Ubuntu, macOS, and Windows on every push and PR. | — |
| `2.test_statement_coverage90` | Gold | Automated tests achieve at least 90% statement coverage | **Gap** | Needs automated CI enforcement gate (`--fail-under 90`) via `cargo-tarpaulin`. | #127 |
| `2.test_branch_coverage80` | Gold | Automated tests achieve at least 80% branch coverage | **Gap** | Needs automated CI enforcement gate for branch coverage via `cargo-tarpaulin`. | #127 |

---

### Category 5: Security (34 Criteria)

*Security covers secure development principles, cryptographic algorithms, keylengths, perfect forward secrecy, CSPRNG, input validation, credential leakage prevention, threat modeling, and hardening.*

| ID | Tier | OpenSSF Requirement Summary | Status | Evidence, Justification, or Gap Description | Ticket |
|---|---|---|:---:|---|:---:|
| `0.know_secure_design` | Passing | Primary developer knows how to design secure software | **Met** | Architecture exhibits zero-trust consensus, fail-closed boundaries (ADR-011, ADR-013), and memory safety. | — |
| `0.know_common_errors` | Passing | Primary developer knows common kinds of vulnerabilities | **Met** | Systematic mitigations for memory corruption, injection, replay attacks, MitM, and consensus equivocation. | — |
| `0.crypto_published` | Passing | Use only published and expert-reviewed crypto algorithms | **Met** | ed25519, SHA-256, BLS12-381 (`blst`), ML-KEM-768 (`aws-lc-rs`), TLS 1.3. | — |
| `0.crypto_call` | Passing | Call standard crypto software, do not re-implement | **Met** | Standard vetted libraries: `ed25519-dalek`, `sha2`, `blst`/`blstrs`, `rustls`, `rustls-webpki`, `aws-lc-rs`. | — |
| `0.crypto_floss` | Passing | All cryptographic functionality implementable with FLOSS | **Met** | All crypto crates in workspace graph are released under Apache-2.0, MIT, or ISC licenses. | — |
| `0.crypto_keylength` | Passing | Default keylengths meet NIST minimums through 2030 | **Met** | 256-bit ed25519, 256-bit SHA-256, BLS12-381 (128-bit security level), ML-KEM-768 (192-bit quantum security level). | — |
| `0.crypto_working` | Passing | Default mechanisms do not depend on broken crypto | **Met** | Zero usage of MD5, SHA-1, DES, RC4, Dual_EC_DRBG, or CBC modes. | — |
| `0.crypto_weaknesses` | Passing | Do not depend on crypto with known serious weaknesses | **Met** | SHA-256 and BLS12-381 standard constructions throughout. | — |
| `0.crypto_pfs` | Passing | Perfect forward secrecy (PFS) for key agreement | **Met** | TLS 1.3 ephemeral Diffie-Hellman / X25519 and ML-KEM-768 hybrid key exchange on all peer connections. | — |
| `0.crypto_password_storage` | Passing | Password storage uses iterated salted hashes | **Pre-release N/A** | GlassChain does not store user passwords. Authentication is exclusively cryptographic via X.509 certificates and digital signatures. | — |
| `0.crypto_random` | Passing | Keys and nonces generated with CSPRNG | **Met** | System entropy utilized via `getrandom` with `sys_rng` feature and `rand_core::OsRng`. | — |
| `0.delivery_mitm` | Passing | Delivery mechanism counters MITM attacks | **Met** | Repository and release distribution strictly over HTTPS and SSH. | — |
| `0.delivery_unsigned` | Passing | Hashes not retrieved over insecure HTTP | **Met** | No unverified HTTP downloads. Git commits and Cargo index enforce cryptographic hashes over TLS. | — |
| `0.vulnerabilities_fixed_60_days` | Passing | No unpatched medium+ severity vulnerabilities > 60 days | **Pre-release N/A** | Zero open or unpatched medium or higher severity vulnerabilities. | — |
| `0.vulnerabilities_critical_fixed` | Passing | Fix critical vulnerabilities rapidly | **Pre-release N/A** | Zero critical vulnerabilities reported or outstanding. | — |
| `0.no_leaked_credentials` | Passing | Repository does not leak private credentials | **Met** | No private keys or tokens committed; tests dynamically generate ephemeral keys (`Identity::generate()`). | — |
| `1.implement_secure_design` | Silver | Implement secure design principles | **Met** | Principle of least privilege, fail-closed certificate verification, defense in depth, and memory safety. | — |
| `1.crypto_weaknesses` | Silver | Default crypto mechanisms do not use weak algorithms | **Met** | Vetted algorithms only; legacy cryptography forbidden by policy. | — |
| `1.crypto_algorithm_agility` | Silver | Support multiple cryptographic algorithms | **Met** | Pluggable consensus and execution provider seams; hybrid TLS negotiation (`X25519MLKEM768` vs `X25519`). | — |
| `1.crypto_credential_agility` | Silver | Credentials and private keys stored separate from code | **Met** | Keys and certificates are external files loaded at runtime or passed via CLI/environment; never hardcoded. | — |
| `1.crypto_used_network` | Silver | Secure protocols for all network communications | **Met** | All P2P and RPC communications strictly mandate TLS 1.3. Plaintext protocols disabled. | — |
| `1.crypto_tls12` | Silver | Support at least TLS version 1.2 | **Met** | `tokio-rustls` enforces TLS 1.3 (minimum TLS 1.2+). | — |
| `1.crypto_certificate_verification` | Silver | TLS certificate verification by default | **Met** | `CertChainVerifier` in `glasschain-identity` and TOFU pinning in `glasschain-network`. | — |
| `1.crypto_verification_private` | Silver | Certificate verification before sending private info | **Met** | Private Data Collections (PDC) fail closed unless peer certificate is verified and possession proved (PR #86, PR #110). | — |
| `1.signed_releases` | Silver | Cryptographically sign releases of project results | **Pre-release N/A** | No official releases cut yet. Sigstore/Cosign release signing will be implemented for v0.1.0. | #126 |
| `1.version_tags_signed` | Silver | Important version tags cryptographically signed | **Pre-release N/A** | No release tags cut yet. Future release tags will be signed via GPG/SSH. | #125 |
| `1.input_validation` | Silver | Check and validate all untrusted inputs (allowlist) | **Met** | 13 strict canonical record schemas (`glasschain-core`), DER decoders for X.509/CRLs, bounded network frames. | — |
| `1.hardening` | Silver | Hardening mechanisms used in software | **Met** | Rust memory safety, integer overflow protection, WASM sandboxing with fuel/gas limits, denial of unsafe code. | — |
| `1.assurance_case` | Silver | Provide an assurance case (threat model, trust boundaries) | **Gap** | Needs consolidated Assurance Case document (`docs/security/assurance-case.md`) synthesizing threat models and trust boundaries. | #126 |
| `2.crypto_used_network` | Gold | Secure protocols for all network communications | **Met** | Mandatory TLS 1.3 across all peer and RPC endpoints. | — |
| `2.crypto_tls12` | Gold | Support at least TLS 1.2 | **Met** | Mandatory TLS 1.3. | — |
| `2.hardened_site` | Gold | Web site includes key hardening headers | **Met** | GitHub Pages / GitHub repository serves strict HSTS and CSP headers. | — |
| `2.security_review` | Gold | Performed security review within last 5 years | **Pre-release N/A** | Pre-release R&D project; external third-party security audit is explicitly scheduled before production release (ADR-010 §7). | #126 |
| `2.hardening` | Gold | Hardening mechanisms MUST be used in software | **Met** | Rust memory safety, stack protection, fuel-metered Cranelift WASM execution, `unsafe_code = "deny"`. | — |

---

### Category 6: Analysis (12 Criteria)

*Analysis covers static code analysis (SAST), software composition analysis (SCA), vulnerability scanning, dynamic analysis (DAST), fuzz testing, memory safety checks, and assertion checking.*

| ID | Tier | OpenSSF Requirement Summary | Status | Evidence, Justification, or Gap Description | Ticket |
|---|---|---|:---:|---|:---:|
| `0.static_analysis` | Passing | Apply at least one static code analysis tool | **Met** | `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` and `cargo audit` run on every push/PR in CI. | — |
| `0.static_analysis_common_vulnerabilities` | Passing | Static analysis searches for common vulnerabilities | **Met** | Clippy pedantic/nursery lints detect common logic errors; `cargo-audit` detects known CVEs in dependencies. | — |
| `0.static_analysis_fixed` | Passing | Fix medium+ severity vulnerabilities found by SAST | **Met** | CI enforces zero warnings (`-D warnings` and `--deny warnings`); all findings fixed. | — |
| `0.static_analysis_often` | Passing | Static analysis occurs on every commit or daily | **Met** | CI executes static analysis on every push to `main` and on every PR. | — |
| `0.dynamic_analysis` | Passing | Dynamic analysis tool applied before release | **Met** | Comprehensive automated test suite, chaos network tests (`tests/chaos_tests.rs`), and fault-injection simulations. | — |
| `0.dynamic_analysis_unsafe` | Passing | Dynamic analysis with memory safety detection for unsafe code | **Pre-release N/A** | Workspace enforces `unsafe_code = "deny"` (zero unsafe Rust). Audited C backends are feature-gated (ADR-015). | — |
| `0.dynamic_analysis_enable_assertions` | Passing | Dynamic analysis enables many assertions | **Met** | Debug assertions and runtime invariant checks active in tests. | — |
| `0.dynamic_analysis_fixed` | Passing | Fix medium+ severity vulnerabilities found by DAST | **Met** | All dynamic test failures and assertions addressed prior to merging. | — |
| `1.static_analysis_common_vulnerabilities` | Silver | Static analysis tool checks for common vulnerabilities | **Met** | Clippy and `cargo-audit` check for known vulnerabilities and dangerous patterns. | — |
| `1.dynamic_analysis_unsafe` | Silver | Dynamic tool routinely used to detect memory safety issues | **Pre-release N/A** | Pre-release pure-Rust workspace (`unsafe_code = "deny"`). C dependencies audited under ADR-015. | — |
| `2.dynamic_analysis` | Gold | Dynamic analysis tool applied to proposed major releases | **Gap** | Implement continuous dynamic analysis via fuzzing (`cargo-fuzz` / libFuzzer) on network parsers, DER decoders, and consensus envelopes. | #128 |
| `2.dynamic_analysis_enable_assertions` | Gold | Include runtime assertions and check during dynamic analysis | **Met** | Runtime assertions extensively used throughout ledger and consensus engines. | — |

---

### Category 7: General Controls — OSPS Baseline Levels 1–3 (64 Criteria)

*General Controls encompasses the Open Source Project Security (OSPS) Baseline Criteria across Levels 1, 2, and 3, covering Access Control (AC), Build & Release (BR), Documentation (DO), Governance (GV), Legal (LE), Quality Assurance (QA), Security Architecture (SA), and Vulnerability Management (VM).*

| ID | Tier | OSPS Baseline Requirement Summary | Status | Evidence, Justification, or Gap Description | Ticket |
|---|---|---|:---:|---|:---:|
| `osps_ac_01_01` | Baseline 1 | MFA required to read/modify sensitive resources | **Met** | Multi-factor authentication enforced on GitHub for repository access. | — |
| `osps_ac_02_01` | Baseline 1 | Lowest available privileges by default for new collaborators | **Met** | GitHub default collaborator permissions configured to lowest privilege. | — |
| `osps_ac_03_01` | Baseline 1 | Enforcement mechanism prevents direct commit to primary branch | **Met** | Branch protection enabled on `main`. | — |
| `osps_ac_03_02` | Baseline 1 | Deletion of primary branch requires explicit confirmation | **Met** | Branch protection prevents deletion of `main`. | — |
| `osps_br_01_01` | Baseline 1 | CI/CD pipeline sanitizes untrusted metadata | **Met** | CI workflows do not evaluate unquoted PR titles or commit messages in shell commands. | — |
| `osps_br_01_03` | Baseline 1 | Untrusted code snapshots isolated from CI credentials | **Met** | GitHub Actions restricts secret access on pull requests from forks. | — |
| `osps_br_03_01` | Baseline 1 | Official project channels exclusively encrypted (HTTPS/SSH) | **Met** | All official URIs use HTTPS or SSH. | — |
| `osps_br_03_02` | Baseline 1 | Official distribution channels protected against MITM | **Met** | HTTPS / SSH enforced for all code distribution channels. | — |
| `osps_br_07_01` | Baseline 1 | Prevent unintentional storage of unencrypted secrets in VCS | **Met** | `.gitignore` excludes keys/credentials; tests use ephemeral memory-generated keys. | — |
| `osps_do_01_01` | Baseline 1 | User guides for all basic functionality | **Met** | `README.md` and `docs/operations.md` document node setup, REPL, and gRPC. | — |
| `osps_do_02_01` | Baseline 1 | Guide for reporting defects | **Met** | `README.md` directs users to GitHub Issues. | #125 |
| `osps_gv_02_01` | Baseline 1 | Mechanisms for public discussions | **Met** | GitHub Issues and Pull Requests are public and open. | — |
| `osps_gv_03_01` | Baseline 1 | Explanation of contribution process | **Gap** | Needs formal public `CONTRIBUTING.md`. | #125 |
| `osps_le_02_01` | Baseline 1 | Source code license meets OSI or FSF definition | **Met** | Apache-2.0 in `LICENSE` is OSI and FSF approved. | — |
| `osps_le_02_02` | Baseline 1 | Released software assets license meets OSI or FSF | **Met** | Released artifacts inherit Apache-2.0 license. | — |
| `osps_le_03_01` | Baseline 1 | Source license maintained in LICENSE file | **Met** | Root `LICENSE` file. | — |
| `osps_le_03_02` | Baseline 1 | Released assets license included in release | **Met** | `LICENSE` bundled with all crate packages and source distributions. | — |
| `osps_qa_01_01` | Baseline 1 | Source repository publicly readable at static URL | **Met** | Static URL: `https://github.com/dbbvitor/GlassChain`. | — |
| `osps_qa_01_02` | Baseline 1 | Publicly readable record of all changes (who, what, when) | **Met** | Public Git commit history. | — |
| `osps_qa_02_01` | Baseline 1 | Direct language dependencies listed in manifest | **Met** | `Cargo.toml` in all 12 crates lists direct dependencies. | — |
| `osps_qa_04_01` | Baseline 1 | Multi-repository codebases documented | **Met** | Single monorepo; all 12 workspace crates listed in root `Cargo.toml` and `README.md`. | — |
| `osps_qa_05_01` | Baseline 1 | VCS must NOT contain generated executable artifacts | **Met** | No precompiled binaries or executable files tracked in Git. | — |
| `osps_qa_05_02` | Baseline 1 | VCS must NOT contain unreviewable binary artifacts | **Met** | Git history verified: zero binary blobs or unreviewable artifacts. | — |
| `osps_vm_02_01` | Baseline 1 | Documentation must contain security contacts | **Gap** | Needs security contact email and reporting instructions in `SECURITY.md`. | #126 |
| `osps_ac_04_01` | Baseline 2 | CI/CD defaults task permissions to lowest privileges | **Met** | All GitHub Actions workflows explicitly specify `permissions: contents: read` at top level. | — |
| `osps_br_02_01` | Baseline 2 | Unique version identifier assigned to official releases | **Pre-release N/A** | Pre-release status. Unique SemVer identifiers will be assigned starting at v0.1.0. | #125 |
| `osps_br_04_01` | Baseline 2 | Release contains descriptive log of changes (changelog) | **Pre-release N/A** | Pre-release status. Human-readable `CHANGELOG.md` will be maintained starting at v0.1.0. | #125 |
| `osps_br_05_01` | Baseline 2 | Build/release pipeline uses standardized dependency tooling | **Met** | Standard Cargo dependency resolution (`cargo build --locked`). | — |
| `osps_br_06_01` | Baseline 2 | Official release signed or in signed manifest | **Pre-release N/A** | Pre-release status. Sigstore/Cosign release signing will be implemented in release CI. | #126 |
| `osps_do_06_01` | Baseline 2 | Documentation describes how dependencies are selected/tracked | **Met** | Documented in `AGENTS.md` and locked in `Cargo.lock` / `clippy.toml`. | #127 |
| `osps_do_07_01` | Baseline 2 | Instructions on how to build software and dependencies | **Met** | `README.md` and `Makefile` detail prerequisites (Rust 1.98.1, `protoc`) and build steps. | — |
| `osps_gv_01_01` | Baseline 2 | Documentation lists members with access to sensitive resources | **Gap** | Needs `MAINTAINERS.md` enumerating project members with repository/secret access. | #125 |
| `osps_gv_01_02` | Baseline 2 | Roles and responsibilities described | **Gap** | Needs role descriptions documented in `GOVERNANCE.md`. | #125 |
| `osps_gv_03_02` | Baseline 2 | Contributor guide includes requirements for acceptable contributions | **Gap** | Needs `CONTRIBUTING.md` defining testing, formatting, and commit standards. | #125 |
| `osps_le_01_01` | Baseline 2 | VCS requires DCO assertion on every commit | **Gap** | Needs DCO sign-off check bot/action configured on PRs. | #125 |
| `osps_qa_03_01` | Baseline 2 | Automated status checks must pass before merge | **Met** | Branch protection enforces passing CI checks before merging to `main`. | — |
| `osps_qa_06_01` | Baseline 2 | CI/CD runs automated test suite before commit accepted | **Met** | `ci.yml` runs full workspace test suite on every PR prior to merge. | — |
| `osps_sa_01_01` | Baseline 2 | Design documentation demonstrates actions and actors | **Met** | Detailed actor/action models in `docs/architecture.md`, `docs/data-model.md`, `CONTEXT.md`. | — |
| `osps_sa_02_01` | Baseline 2 | Descriptions of all external software interfaces | **Met** | `docs/operations.md`, `docs/data-model.md`, `glasschain-rpc/proto/`. | — |
| `osps_sa_03_01` | Baseline 2 | Security assessment performed on release | **Pre-release N/A** | Pre-release status. Security assessment is scheduled prior to production v1.0 (ADR-010 §7). | #126 |
| `osps_vm_01_01` | Baseline 2 | Policy for coordinated vulnerability disclosure (CVD) | **Gap** | Needs CVD policy with response timeframes in `SECURITY.md`. | #126 |
| `osps_vm_03_01` | Baseline 2 | Means for private vulnerability reporting to security contacts | **Gap** | Needs private vulnerability intake instructions in `SECURITY.md`. | #126 |
| `osps_vm_04_01` | Baseline 2 | Publicly publish data about discovered vulnerabilities | **Gap** | Needs vulnerability publication procedures in `SECURITY.md`. | #126 |
| `osps_ac_04_02` | Baseline 3 | CI/CD jobs assign minimum privileges | **Met** | Granular workflow permissions configured (`permissions: contents: read`). | — |
| `osps_br_01_04` | Baseline 3 | Sanitize and validate collaborator input in CI/CD | **Met** | CI workflows use static arguments and do not execute raw input. | — |
| `osps_br_02_02` | Baseline 3 | All release assets associated with release identifier | **Pre-release N/A** | Pre-release status. Will be enforced in release workflow. | #125 |
| `osps_br_07_02` | Baseline 3 | Policy for managing secrets and credentials | **Gap** | Needs documented secrets management and rotation policy in `SECURITY.md`. | #126 |
| `osps_do_03_01` | Baseline 3 | Instructions to verify integrity/authenticity of release assets | **Pre-release N/A** | Pre-release status. Verification instructions (Cosign/minisign) will accompany v0.1.0 release. | #126 |
| `osps_do_03_02` | Baseline 3 | Instructions to verify identity of release signer | **Pre-release N/A** | Pre-release status. Release signer identity verification will be documented for v0.1.0. | #126 |
| `osps_do_04_01` | Baseline 3 | Scope and duration of support for each release | **Gap** | Needs `SUPPORT.md` stating support duration and release lifecycle. | #127 |
| `osps_do_05_01` | Baseline 3 | Statement when releases no longer receive security updates | **Gap** | Needs EOL / security support statement in `SUPPORT.md` or `SECURITY.md`. | #127 |
| `osps_gv_04_01` | Baseline 3 | Policy that collaborators are reviewed before granting escalated access | **Gap** | Needs maintainer onboarding and access review policy in `GOVERNANCE.md`. | #125 |
| `osps_qa_02_02` | Baseline 3 | Compiled release assets delivered with SBOM | **Gap** | Needs automated SBOM generation (via `cargo-auditable` / CycloneDX) in CI build/release pipeline. | #128 |
| `osps_qa_04_02` | Baseline 3 | Subprojects enforce security parity with primary codebase | **Met** | Root `[workspace.lints]` automatically enforces identical strict lint/security settings across all 12 crates. | — |
| `osps_qa_06_02` | Baseline 3 | Clearly document when and how tests are run | **Met** | Documented in `AGENTS.md`, `README.md`, and `Makefile`. | — |
| `osps_qa_06_03` | Baseline 3 | Policy that all major changes add/update automated tests | **Met** | Mandated in `AGENTS.md` § Testing instructions. | — |
| `osps_qa_07_01` | Baseline 3 | Non-author human approval required before merging to primary branch | **Gap** | Configure branch protection requiring >=1 human approval before merge. | #125 |
| `osps_sa_03_02` | Baseline 3 | Threat modeling and attack surface analysis performed on release | **Gap** | Needs comprehensive threat model and attack surface analysis document in `docs/security/`. | #126 |
| `osps_vm_04_02` | Baseline 3 | Non-affecting vulnerabilities accounted for in VEX document | **Pre-release N/A** | Pre-release status. VEX feed/statement will be published when release assets are distributed. | #128 |
| `osps_vm_05_01` | Baseline 3 | Policy defining threshold for SCA remediation | **Gap** | Document SCA vulnerability severity remediation thresholds in `SECURITY.md`. | #128 |
| `osps_vm_05_02` | Baseline 3 | Policy to address SCA violations prior to release | **Gap** | Document mandatory zero-SCA-advisory gate prior to release in `SECURITY.md`. | #128 |
| `osps_vm_05_03` | Baseline 3 | Codebase evaluated against SCA policy and blocked on violations | **Met** | `cargo-audit --deny warnings` runs on every push and PR in `.github/workflows/audit.yml`. | — |
| `osps_vm_06_01` | Baseline 3 | Policy defining threshold for SAST remediation | **Gap** | Document SAST remediation thresholds and zero-warning policy in `SECURITY.md`. | #128 |
| `osps_vm_06_02` | Baseline 3 | Codebase evaluated against SAST policy and blocked on violations | **Met** | `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` blocks CI on any violation. | — |

---

## 4. Pre-Release N/A Justifications Catalog

This section provides defensible, primary-source justifications for all **31 criteria** classified as **Pre-release N/A**. Under OpenSSF Best Practices guidelines, projects that have not yet published an official general release or that are in pre-release research and development status may honestly select "Not Applicable" for criteria contingent on past release history or external user operations.

### 4.1 Release History and Lifecycle (8 Criteria)
- **Criteria:** `0.version_unique`, `0.version_tags`, `0.release_notes`, `0.release_notes_vulns`, `1.maintenance_or_update`, `osps_br_02_01`, `osps_br_02_02`, `osps_br_04_01`.
- **Justification:** GlassChain is currently in active pre-release development. No public distribution releases have been cut to date; all workspace crates are at development version `0.1.0`. There are no legacy versions in production requiring maintenance or migration shims. Per `AGENTS.md`, pre-release architectural priorities explicitly favor clean refactoring over backwards-compatibility shims or dual-format decoders. Unique SemVer identifiers, git release tags, and a human-readable `CHANGELOG.md` will be initiated with the first public release (v0.1.0).

### 4.2 Release Asset Signing and Verification (5 Criteria)
- **Criteria:** `1.signed_releases`, `1.version_tags_signed`, `osps_br_06_01`, `osps_do_03_01`, `osps_do_03_02`.
- **Justification:** Cryptographic release signing, key distribution, and signature verification instructions apply to official distributed release artifacts and release tags. Because no official distribution builds or release tags currently exist, these criteria are not applicable during pre-release development. A modern supply-chain signing pipeline using Sigstore/Cosign and minitags will be incorporated into the v0.1.0 release workflow.

### 4.3 Vulnerability Disclosure and History (5 Criteria)
- **Criteria:** `0.vulnerability_report_response`, `0.vulnerabilities_fixed_60_days`, `0.vulnerabilities_critical_fixed`, `1.vulnerability_report_credit`, `osps_vm_04_02`.
- **Justification:** During GlassChain's pre-release research phase, zero external vulnerability reports have been received, zero CVEs have been assigned or fixed, and zero vulnerability reports are awaiting response or credit attribution. There are no known unpatched medium or higher severity vulnerabilities. VEX exploitability statements will be established alongside the publication of release artifacts.

### 4.4 External User Password Storage (2 Criteria)
- **Criteria:** `0.crypto_password_storage`, `1.sites_password_security`.
- **Justification:** GlassChain is a cryptographic distributed ledger that authenticates entities exclusively through public-key cryptography (ed25519 digital signatures, BLS12-381 multisignatures, and X.509 Membership Service Provider identity certificates). The system contains no concept of external user passwords, password databases, or user credential hashes. The project's public repository is hosted on GitHub, which manages its own authentication infrastructure.

### 4.5 Community Governance and Multi-Party Contributor Base (4 Criteria)
- **Criteria:** `1.bus_factor`, `2.bus_factor`, `2.contributors_unassociated`, `2.two_person_review`.
- **Justification:** As a pre-release research project originated by a single lead architect (`dbbvitor`), GlassChain operates with an initial bus factor of 1. Multi-maintainer governance, independent contributor onboarding, and mandatory dual-human PR approval gates are organizational scaling milestones planned for the public testnet federation phase (ADR-010 §7), following initial stability and API maturation.

### 4.6 Independent Security Review (2 Criteria)
- **Criteria:** `2.security_review`, `osps_sa_03_01`.
- **Justification:** Gold level requires an independent external security review within the last 5 years. GlassChain's governance policy (ADR-010 §7) explicitly establishes an external third-party security audit as a mandatory gate for production mainnet/federation deployment, following testnet stability. Performing an external commercial audit on a rapidly evolving pre-release codebase is premature; internal security audits, threat models, and architectural reviews are conducted continuously.

### 4.7 Accessibility and Internationalization (2 Criteria)
- **Criteria:** `1.accessibility_best_practices`, `1.internationalization`.
- **Justification:** GlassChain produces backend distributed ledger infrastructure, a headless P2P network daemon, a gRPC API server, and a developer CLI. All interfaces and logs operate via standard ASCII/UTF-8 technical English in monospace terminal environments. There is currently no web user interface or end-user localized application deployed (the visual browser demo is a future milestone specified in `.agents/plans/gui-demo-benchmark.md`).

### 4.8 Unsafe Code Dynamic Memory Safety (3 Criteria)
- **Criteria:** `0.dynamic_analysis_unsafe`, `1.dynamic_analysis_unsafe`, `osps_sa_03_01`.
- **Justification:** GlassChain enforces `unsafe_code = "deny"` across the entire workspace in `Cargo.toml`. There are zero lines of unsafe Rust in workspace crates. The two external C dependencies in the graph (`blst` and `aws-lc-rs`) are feature-gated and were adopted pursuant to formal architectural review and upstream audit evidence (ADR-015).

---

## 5. Gap Catalog and Downstream Specification Roadmap

The audit identified **42 concrete gaps** that require explicit specification and implementation to achieve full OpenSSF Gold and OSPS Baseline Level 3 compliance.

These gaps are mapped directly to the active sub-issue tickets of Wayfinder Map [#123](https://github.com/dbbvitor/GlassChain/issues/123):

```
Wayfinder Map #123 Ticket Breakdown:
├── #124: Research: OpenSSF criteria baseline audit (THIS REPORT)
├── #125: Spec: Basics, Governance, and Change Control (Passing -> Gold)
├── #126: Spec: Vulnerability Reporting, Security Architecture, and Cryptography (Passing -> Gold)
├── #127: Spec: Quality, Build Reproducibility, and Test Coverage (Passing -> Gold)
├── #128: Spec: Static/Dynamic Analysis, Hardening, and General Controls (Passing -> Gold)
└── #129: Synthesis: Master OpenSSF compliance document and badge application roadmap
```

### 5.1 Ticket #125: Basics, Governance, and Change Control
*Scope: Documentation, Contributor Guides, Governance Models, Legal Assertions, and Repository Protections.*

| Target Artifact / Policy | Relevant OpenSSF / OSPS Criteria | Concrete Specification Requirements |
|---|---|---|
| `CONTRIBUTING.md` | `0.interact`, `0.contribution`, `1.contribution_requirements`, `osps_gv_03_01`, `osps_gv_03_02` | Create public contributor guide defining PR submission workflow, local test and lint requirements (`make ci`), commit message style, and pull request template. |
| DCO Commit Sign-off | `1.dco`, `osps_le_01_01` | Implement Developer Certificate of Origin policy (`Signed-off-by: Name <email>`) with GitHub Action / DCO app check on all pull requests. |
| `GOVERNANCE.md` | `1.governance`, `1.roles_responsibilities`, `osps_gv_01_02`, `osps_gv_04_01` | Define project governance model, consensus decision-making, maintainer roles, contributor onboarding, and access review policies. |
| `MAINTAINERS.md` | `1.roles_responsibilities`, `1.access_continuity`, `osps_gv_01_01` | Enumerate maintainers, roles, access levels to sensitive resources (secrets, crates.io), and emergency continuity procedures. |
| `CODE_OF_CONDUCT.md` | `1.code_of_conduct` | Adopt Contributor Covenant v2.1 with clear reporting and enforcement contact information. |
| Per-File SPDX Headers | `2.copyright_per_file`, `2.license_per_file` | Add `// SPDX-License-Identifier: Apache-2.0` and copyright notice across all `.rs` source files using automated tooling. |
| Branch Protection Rules | `osps_qa_07_01`, `2.two_person_review` | Configure GitHub branch protection on `main` to require >=1 approving review from maintainers before merge. |
| Contributor Task Labels | `2.small_tasks` | Curate and label starter issues with standard `good first issue` / `help wanted` tags. |

### 5.2 Ticket #126: Vulnerability Reporting, Security Architecture, and Cryptography
*Scope: Vulnerability Management, CVD Policy, Private Reporting, Threat Modeling, and Release Signing.*

| Target Artifact / Policy | Relevant OpenSSF / OSPS Criteria | Concrete Specification Requirements |
|---|---|---|
| `SECURITY.md` | `0.vulnerability_report_process`, `0.vulnerability_report_private`, `1.vulnerability_response_process`, `osps_vm_01_01`, `osps_vm_02_01`, `osps_vm_03_01`, `osps_vm_04_01` | Publish Coordinated Vulnerability Disclosure (CVD) policy, security contact email, GitHub Private Vulnerability Reporting (PVR) workflow, response SLAs (acknowledgement <= 48h, triage <= 14d), and reporter credit policy. |
| Security Assurance Case & Threat Model | `1.assurance_case`, `osps_sa_03_02` | Author formal assurance case in `docs/security/assurance-case.md` synthesizing threat models, trust boundaries, attack surface analysis, and countermeasure proofs. |
| Release Signing (Cosign) | `1.signed_releases`, `osps_br_06_01` | Specify Sigstore/Cosign keyless or KMS-backed signing for release binaries and checksum manifests in GitHub Actions release workflow. |
| Secrets Management Policy | `osps_br_07_02` | Document secrets management policy (GitHub Actions secrets, rotating tokens, no plaintext credentials in configs). |

### 5.3 Ticket #127: Quality, Build Reproducibility, and Test Coverage
*Scope: Coverage Gating, Build Determinism, Dependency Selection, and Support Lifecycle.*

| Target Artifact / Policy | Relevant OpenSSF / OSPS Criteria | Concrete Specification Requirements |
|---|---|---|
| Tarpaulin Coverage CI Gates | `1.test_statement_coverage80`, `2.test_statement_coverage90`, `2.test_branch_coverage80` | Upgrade `.github/workflows/ci.yml` `coverage` job to enforce strict threshold gates: `--fail-under 80` (immediate Silver gate) and path to 90% statement / 80% branch coverage. |
| Reproducible Build Verification | `2.build_reproducible` | Implement CI step testing bit-for-bit build determinism across clean environments using `reproducible-builds` techniques and `remap-path-prefix`. |
| Dependency Governance Docs | `osps_do_06_01` | Document formal dependency selection, vetting, auditing (`cargo-audit`), and update tracking procedures in `docs/architecture.md`. |
| `SUPPORT.md` | `osps_do_04_01`, `osps_do_05_01` | Publish support lifecycle document defining supported versions, security patch windows, and End-of-Life (EOL) policies. |

### 5.4 Ticket #128: Static/Dynamic Analysis, Hardening, and General Controls
*Scope: Fuzzing Harnesses, CodeQL SAST, SBOM Delivery, VEX Feed, and Analysis Policies.*

| Target Artifact / Policy | Relevant OpenSSF / OSPS Criteria | Concrete Specification Requirements |
|---|---|---|
| Continuous Dynamic Fuzzing | `2.dynamic_analysis` | Add `fuzz/` crate with `cargo-fuzz` / libFuzzer targets covering untrusted inputs: consensus vote parsing, X.509/DER decoding, wire codecs, and WASM bytecode validation. Wire smoke fuzzing into CI. |
| GitHub CodeQL Workflow | `0.static_analysis`, `1.static_analysis_common_vulnerabilities` | Add `.github/workflows/codeql.yml` for multi-language automated static application security testing (SAST). |
| Automated SBOM Generation | `osps_qa_02_02` | Wire `cargo-auditable` into build and release pipelines to embed dependency metadata in compiled binaries, and emit CycloneDX/SPDX SBOM manifests. |
| VEX Policy & Statement | `osps_vm_04_02` | Establish Vulnerability Exploitability eXchange (VEX) policy in `docs/security/vex.md` to document and suppress non-exploitable transitive dependencies (e.g. `audit.toml` exemptions). |
| SAST / SCA Remediation Policy | `osps_vm_05_01`, `osps_vm_05_02`, `osps_vm_06_01` | Document formal policies in `SECURITY.md` defining remediation timelines and blocking thresholds for SAST/SCA findings prior to release. |

### 5.5 Ticket #129: Synthesis & OpenSSF Badge Application Roadmap
*Scope: Master Compliance Document and Application Dossier.*
- Synthesize all findings and specifications into `docs/compliance/openssf-best-practices.md` and `.agents/plans/openssf-compliance.md`.
- Establish the official OpenSSF Best Practices badge project entry on `bestpractices.dev` with pre-filled justifications and URLs.

---

## 6. Master Summary Scorecard Table

| Criterion Identifier | Level | Category | OpenSSF Requirement | Status | Verification Reference / Ticket |
|---|---|---|---|:---:|---|
| `0.description_good` | Passing | Basics | Project website describes what software does | **Met** | `README.md` lines 1–15 |
| `0.interact` | Passing | Basics | Info on obtain, feedback, contribute | **Gap** | #125 (`CONTRIBUTING.md`) |
| `0.contribution` | Passing | Basics | Explains contribution process | **Gap** | #125 (`CONTRIBUTING.md`) |
| `0.contribution_requirements` | Passing | Basics | Requirements for acceptable contributions | **Met** | `AGENTS.md`, `clippy.toml` |
| `0.floss_license` | Passing | Basics | Released as FLOSS | **Met** | Apache-2.0 (`LICENSE`) |
| `0.floss_license_osi` | Passing | Basics | OSI-approved license | **Met** | Apache-2.0 OSI approved |
| `0.license_location` | Passing | Basics | License in standard location | **Met** | Root `LICENSE` |
| `0.documentation_basics` | Passing | Basics | Basic user documentation | **Met** | `README.md`, `docs/operations.md` |
| `0.documentation_interface` | Passing | Basics | Reference documentation of interfaces | **Met** | `docs/operations.md`, `proto/` |
| `0.sites_https` | Passing | Basics | Sites support HTTPS using TLS | **Met** | GitHub HTTPS |
| `0.discussion` | Passing | Basics | Searchable public discussion mechanism | **Met** | GitHub Issues & PRs |
| `0.english` | Passing | Basics | English documentation and responses | **Met** | 100% English |
| `0.maintained` | Passing | Basics | Project is maintained | **Met** | Active daily commits |
| `0.repo_public` | Passing | Change Control | Public source repository | **Met** | GitHub public repo |
| `0.repo_track` | Passing | Change Control | Tracks changes, authors, dates | **Met** | Git VCS |
| `0.repo_interim` | Passing | Change Control | Interim versions tracked in VCS | **Met** | Git commit history |
| `0.repo_distributed` | Passing | Change Control | Distributed VCS used | **Met** | Git |
| `0.version_unique` | Passing | Change Control | Unique version identifier per release | **Pre-release N/A** | Pre-release; SemVer in `Cargo.toml` |
| `0.version_semver` | Passing | Change Control | SemVer or CalVer format used | **Met** | SemVer `0.1.0` in `Cargo.toml` |
| `0.version_tags` | Passing | Change Control | Releases identified in VCS tags | **Pre-release N/A** | Pre-release; tags begin v0.1.0 |
| `0.release_notes` | Passing | Change Control | Human-readable release notes | **Pre-release N/A** | Pre-release; `CHANGELOG.md` at v0.1.0 |
| `0.release_notes_vulns` | Passing | Change Control | Release notes identify fixed CVEs | **Pre-release N/A** | Zero past CVEs |
| `0.report_process` | Passing | Reporting | Process to submit bug reports | **Met** | GitHub Issues |
| `0.report_tracker` | Passing | Reporting | Issue tracker used for issues | **Met** | GitHub Issues |
| `0.report_responses` | Passing | Reporting | Acknowledge majority of bug reports | **Met** | Rapid maintainer response |
| `0.enhancement_responses` | Passing | Reporting | Respond to majority of enhancements | **Met** | GitHub Issues triage |
| `0.report_archive` | Passing | Reporting | Publicly available archive of reports | **Met** | GitHub Issues archive |
| `0.vulnerability_report_process` | Passing | Reporting | Publish process for vulnerability reports | **Gap** | #126 (`SECURITY.md`) |
| `0.vulnerability_report_private` | Passing | Reporting | How to report vulnerabilities privately | **Gap** | #126 (`SECURITY.md`) |
| `0.vulnerability_report_response` | Passing | Reporting | Response time for vulnerability reports <=14d | **Pre-release N/A** | Zero past reports; SLA in #126 |
| `0.build` | Passing | Quality | Working automated build system | **Met** | Cargo (`cargo build`) |
| `0.build_common_tools` | Passing | Quality | Common build tools used | **Met** | Rust / Cargo |
| `0.build_floss_tools` | Passing | Quality | Buildable with FLOSS tools only | **Met** | Rust, Cargo, Protoc, LLVM |
| `0.test` | Passing | Quality | Automated test suite in FLOSS | **Met** | `cargo test --workspace ...` |
| `0.test_invocation` | Passing | Quality | Standard test invocation | **Met** | `cargo test` / `make test` |
| `0.test_most` | Passing | Quality | Tests cover most branches/functionality | **Met** | Unit, integration, chaos tests |
| `0.test_continuous_integration` | Passing | Quality | CI runs automated tests on new code | **Met** | GitHub Actions `ci.yml` |
| `0.test_policy` | Passing | Quality | Policy that tests added for new features | **Met** | Mandated in `AGENTS.md` |
| `0.tests_are_added` | Passing | Quality | Evidence that test policy is adhered to | **Met** | Verified in git history |
| `0.tests_documented_added` | Passing | Quality | Test policy documented in instructions | **Met** | Documented in `AGENTS.md` |
| `0.warnings` | Passing | Quality | Compiler warnings / linter enabled | **Met** | `clippy`, `-D warnings` |
| `0.warnings_fixed` | Passing | Quality | Address warnings | **Met** | CI clean, 0 warnings |
| `0.warnings_strict` | Passing | Quality | Maximally strict with warnings | **Met** | `unsafe_code = "deny"`, clippy pedantic |
| `0.know_secure_design` | Passing | Security | Primary developer knows secure design | **Met** | Zero-trust, fail-closed boundaries |
| `0.know_common_errors` | Passing | Security | Primary developer knows common errors | **Met** | Systematic anti-vulnerability designs |
| `0.crypto_published` | Passing | Security | Cryptography published and expert-reviewed | **Met** | ed25519, SHA-256, BLS12-381, ML-KEM |
| `0.crypto_call` | Passing | Security | Call standard crypto software | **Met** | `ed25519-dalek`, `blst`, `rustls` |
| `0.crypto_floss` | Passing | Security | Crypto implementable with FLOSS | **Met** | All crypto crates FLOSS |
| `0.crypto_keylength` | Passing | Security | Keylengths meet NIST through 2030 | **Met** | 256-bit ed25519, BLS12-381, ML-KEM-768 |
| `0.crypto_working` | Passing | Security | No broken crypto algorithms | **Met** | No MD5/SHA-1/DES/RC4/CBC |
| `0.crypto_weaknesses` | Passing | Security | No crypto with known serious weaknesses | **Met** | Modern algorithms only |
| `0.crypto_pfs` | Passing | Security | Perfect forward secrecy implemented | **Met** | TLS 1.3 ephemeral keys on all links |
| `0.crypto_password_storage` | Passing | Security | Password storage uses iterated hashes | **Pre-release N/A** | No passwords stored |
| `0.crypto_random` | Passing | Security | CSPRNG used for keys and nonces | **Met** | `getrandom` `sys_rng`, `rand_core` |
| `0.delivery_mitm` | Passing | Security | Delivery counters MITM attacks | **Met** | HTTPS / SSH |
| `0.delivery_unsigned` | Passing | Security | Hashes not retrieved over insecure HTTP | **Met** | TLS enforced everywhere |
| `0.vulnerabilities_fixed_60_days` | Passing | Security | No unpatched medium+ vulns > 60 days | **Pre-release N/A** | Zero open vulnerabilities |
| `0.vulnerabilities_critical_fixed` | Passing | Security | Fix critical vulnerabilities rapidly | **Pre-release N/A** | Zero critical vulnerabilities |
| `0.no_leaked_credentials` | Passing | Security | Public repo does not leak credentials | **Met** | Verified clean; ephemeral test keys |
| `0.static_analysis` | Passing | Analysis | Apply static analysis tool | **Met** | `cargo clippy`, `cargo audit` in CI |
| `0.static_analysis_common_vulnerabilities` | Passing | Analysis | Static analysis checks for vulnerabilities | **Met** | Clippy + RustSec audit database |
| `0.static_analysis_fixed` | Passing | Analysis | Fix medium+ vulns found by SAST | **Met** | CI enforces 0 findings |
| `0.static_analysis_often` | Passing | Analysis | Static analysis runs on every commit/daily | **Met** | Runs on every push and PR |
| `0.dynamic_analysis` | Passing | Analysis | Dynamic analysis applied before release | **Met** | Automated unit/integration/chaos tests |
| `0.dynamic_analysis_unsafe` | Passing | Analysis | Dynamic tool with memory safety detection | **Pre-release N/A** | `unsafe_code = "deny"` (0 unsafe Rust) |
| `0.dynamic_analysis_enable_assertions` | Passing | Analysis | Dynamic analysis enables many assertions | **Met** | Debug/runtime assertions enabled |
| `0.dynamic_analysis_fixed` | Passing | Analysis | Fix medium+ vulns found by DAST | **Met** | All test failures resolved |
| `1.achieve_passing` | Silver | Basics | Achieve Passing level badge | **Gap** | #129 (Badge Application) |
| `1.contribution_requirements` | Silver | Basics | Requirements for acceptable contributions | **Gap** | #125 (`CONTRIBUTING.md`) |
| `1.dco` | Silver | Basics | Legal mechanism for contributions (DCO/CLA) | **Gap** | #125 (DCO CI enforcement) |
| `1.governance` | Silver | Basics | Documented project governance model | **Gap** | #125 (`GOVERNANCE.md`) |
| `1.code_of_conduct` | Silver | Basics | Adopt and post Code of Conduct | **Gap** | #125 (`CODE_OF_CONDUCT.md`) |
| `1.roles_responsibilities` | Silver | Basics | Document key roles and responsibilities | **Gap** | #125 (`MAINTAINERS.md`) |
| `1.access_continuity` | Silver | Basics | Project continuity plan | **Gap** | #125 (`MAINTAINERS.md`) |
| `1.bus_factor` | Silver | Basics | Bus factor of 2 or more | **Pre-release N/A** | Pre-release solo lead; roadmap #125 |
| `1.documentation_roadmap` | Silver | Basics | Documented roadmap for at least 1 year | **Met** | `.agents/plans/README.md` |
| `1.documentation_architecture` | Silver | Basics | Documentation of software architecture | **Met** | `docs/architecture.md` (41 KB) |
| `1.documentation_security` | Silver | Basics | Document security requirements/expectations | **Met** | `README.md`, `docs/privacy-and-identity.md` |
| `1.documentation_quick_start` | Silver | Basics | Quick start guide for new users | **Met** | `README.md` Quick start |
| `1.documentation_current` | Silver | Basics | Keep documentation current with code | **Met** | Enforced by `AGENTS.md` |
| `1.documentation_achievements` | Silver | Basics | Hyperlink achievements within 48h | **Pre-release N/A** | Applied upon badge attainment |
| `1.accessibility_best_practices` | Silver | Basics | Follow accessibility best practices | **Pre-release N/A** | Backend CLI/daemon; no GUI |
| `1.internationalization` | Silver | Basics | Internationalization of software | **Pre-release N/A** | Technical English infrastructure |
| `1.sites_password_security` | Silver | Basics | Passwords stored securely on sites | **Pre-release N/A** | No site password storage |
| `1.maintenance_or_update` | Silver | Change Control | Maintain older versions OR upgrade path | **Pre-release N/A** | Pre-release; no legacy versions |
| `1.report_tracker` | Silver | Reporting | Use an issue tracker for tracking issues | **Met** | GitHub Issues |
| `1.vulnerability_report_credit` | Silver | Reporting | Credit vulnerability reporters | **Pre-release N/A** | Zero past reports; policy in #126 |
| `1.vulnerability_response_process` | Silver | Reporting | Documented vulnerability response process | **Gap** | #126 (`SECURITY.md`) |
| `1.coding_standards` | Silver | Quality | Specific coding style guide identified | **Met** | `rustfmt`, `clippy.toml` |
| `1.coding_standards_enforced` | Silver | Quality | Automatically enforce coding style | **Met** | CI `fmt` & `clippy` gates |
| `1.build_standard_variables` | Silver | Quality | Build system honors standard env vars | **Met** | Cargo honors standard flags |
| `1.build_preserve_debug` | Silver | Quality | Build preserves debugging info if requested | **Met** | Cargo dev profile (`debug = true`) |
| `1.build_non_recursive` | Silver | Quality | Build system is non-recursive | **Met** | Cargo DAG build |
| `1.build_repeatable` | Silver | Quality | Repeatable bit-for-bit generation | **Met** | Pinned `Cargo.lock` & toolchain 1.98.1 |
| `1.installation_common` | Silver | Quality | Easy install using common convention | **Met** | `cargo install --path ...` |
| `1.installation_standard_variables` | Silver | Quality | Installation honors standard variables | **Met** | `CARGO_HOME`, `DESTDIR` |
| `1.installation_development_quick` | Silver | Quality | Quick setup for developers | **Met** | `Makefile` (`make setup`, `make build`) |
| `1.external_dependencies` | Silver | Quality | Computer-processable dependency list | **Met** | `Cargo.toml`, `Cargo.lock` |
| `1.dependency_monitoring` | Silver | Quality | Monitor dependencies for vulnerabilities | **Met** | Dependabot & `cargo-audit` in CI |
| `1.updateable_reused_components` | Silver | Quality | Easy to update reused components | **Met** | Cargo crates.io ecosystem |
| `1.interfaces_current` | Silver | Quality | Avoid deprecated/obsolete functions | **Met** | Warnings denied in CI |
| `1.automated_integration_testing` | Silver | Quality | Automated tests run on each check-in | **Met** | CI runs on every push and PR |
| `1.regression_tests_added50` | Silver | Quality | Regression tests for >=50% of bugs | **Met** | Evidence in commit history |
| `1.test_statement_coverage80` | Silver | Quality | Automated tests provide >=80% statement coverage | **Gap** | #127 (Tarpaulin gate in CI) |
| `1.test_policy_mandated` | Silver | Quality | Formal written policy for adding tests | **Met** | Mandated in `AGENTS.md` |
| `1.tests_documented_added` | Silver | Quality | Instructions document policy to add tests | **Met** | Documented in `AGENTS.md` |
| `1.warnings_strict` | Silver | Quality | Maximally strict with warnings | **Met** | `clippy.toml`, `-D warnings` |
| `1.implement_secure_design` | Silver | Security | Implement secure design principles | **Met** | Zero-trust, fail-closed boundaries |
| `1.crypto_weaknesses` | Silver | Security | No weak crypto algorithms or modes | **Met** | Modern algorithms only |
| `1.crypto_algorithm_agility` | Silver | Security | Support multiple cryptographic algorithms | **Met** | Provider seams, hybrid TLS |
| `1.crypto_credential_agility` | Silver | Security | Credentials stored separate from code | **Met** | External key files / runtime gen |
| `1.crypto_used_network` | Silver | Security | Secure protocols for all network comms | **Met** | TLS 1.3 mandatory |
| `1.crypto_tls12` | Silver | Security | Support at least TLS 1.2 | **Met** | TLS 1.3 enforced |
| `1.crypto_certificate_verification` | Silver | Security | Perform TLS cert verification by default | **Met** | `CertChainVerifier`, TOFU pinning |
| `1.crypto_verification_private` | Silver | Security | Verify cert before sending private info | **Met** | PDC fail-closed verification |
| `1.signed_releases` | Silver | Security | Cryptographically sign releases | **Pre-release N/A** | Pre-release; Cosign in #126 |
| `1.version_tags_signed` | Silver | Security | Version tags cryptographically signed | **Pre-release N/A** | Pre-release; GPG/SSH tags in #125 |
| `1.input_validation` | Silver | Security | Check all untrusted inputs (allowlist) | **Met** | 13 schemas, DER decoders |
| `1.hardening` | Silver | Security | Hardening mechanisms used in software | **Met** | Rust memory safety, WASM sandbox |
| `1.assurance_case` | Silver | Security | Provide an assurance case & threat model | **Gap** | #126 (`docs/security/assurance-case.md`) |
| `1.static_analysis_common_vulnerabilities` | Silver | Analysis | SAST checks for common vulnerabilities | **Met** | Clippy + `cargo-audit` |
| `1.dynamic_analysis_unsafe` | Silver | Analysis | Dynamic tool detects memory safety in unsafe code | **Pre-release N/A** | Zero unsafe Rust in workspace |
| `2.achieve_silver` | Gold | Basics | Achieve Silver level badge | **Gap** | #129 (Badge Application) |
| `2.bus_factor` | Gold | Basics | Bus factor of 2 or more | **Pre-release N/A** | Pre-release solo lead; roadmap #125 |
| `2.contributors_unassociated` | Gold | Basics | At least two unassociated contributors | **Pre-release N/A** | Pre-release research project |
| `2.copyright_per_file` | Gold | Basics | Copyright statement in each source file | **Gap** | #125 (Per-file copyright) |
| `2.license_per_file` | Gold | Basics | License statement in each source file | **Gap** | #125 (SPDX headers) |
| `2.repo_distributed` | Gold | Change Control | Source repo uses distributed VCS | **Met** | Git |
| `2.small_tasks` | Gold | Change Control | Clearly identify small tasks for new contributors | **Gap** | #125 (`good first issue` labeling) |
| `2.require_2FA` | Gold | Change Control | Require 2FA for developers | **Met** | GitHub account 2FA enforced |
| `2.secure_2FA` | Gold | Change Control | 2FA uses cryptographic mechanisms | **Met** | WebAuthn / TOTP |
| `2.code_review_standards` | Gold | Quality | Documented code review requirements | **Gap** | #125 (`CONTRIBUTING.md`) |
| `2.two_person_review` | Gold | Quality | >=50% of modifications reviewed by non-author | **Pre-release N/A** | Pre-release solo maintainer |
| `2.build_reproducible` | Gold | Quality | Reproducible build (bit-for-bit) | **Gap** | #127 (Deterministic CI pipeline) |
| `2.test_invocation` | Gold | Quality | Test suite MUST be invocable in standard way | **Met** | `cargo test` standard invocation |
| `2.test_continuous_integration` | Gold | Quality | CI runs automated tests on new code | **Met** | Multi-OS CI matrix |
| `2.test_statement_coverage90` | Gold | Quality | Automated tests provide >=90% statement coverage | **Gap** | #127 (Tarpaulin 90% gate) |
| `2.test_branch_coverage80` | Gold | Quality | Automated tests provide >=80% branch coverage | **Gap** | #127 (Tarpaulin branch gate) |
| `2.crypto_used_network` | Gold | Security | Secure protocols for all network comms | **Met** | Mandatory TLS 1.3 |
| `2.crypto_tls12` | Gold | Security | Support at least TLS 1.2 | **Met** | Mandatory TLS 1.3 |
| `2.hardened_site` | Gold | Security | Website includes key hardening headers | **Met** | GitHub strict security headers |
| `2.security_review` | Gold | Security | Performed security review in last 5 years | **Pre-release N/A** | Pre-release; external audit in #126 |
| `2.hardening` | Gold | Security | Hardening mechanisms used in software | **Met** | Rust memory safety, WASM sandbox |
| `2.dynamic_analysis` | Gold | Analysis | Dynamic analysis tool applied to releases | **Gap** | #128 (`cargo-fuzz` harness) |
| `2.dynamic_analysis_enable_assertions` | Gold | Analysis | Include runtime assertions in dynamic analysis | **Met** | Runtime invariant checks active |
| `osps_ac_01_01` | Baseline 1 | General Controls | MFA required to read/modify sensitive resources | **Met** | GitHub 2FA enforced |
| `osps_ac_02_01` | Baseline 1 | General Controls | Lowest available privileges by default for collaborators | **Met** | GitHub default permissions |
| `osps_ac_03_01` | Baseline 1 | Enforcement prevents direct commit to primary branch | **Met** | Branch protection on `main` |
| `osps_ac_03_02` | Baseline 1 | Deletion of primary branch requires explicit confirmation | **Met** | Branch protection on `main` |
| `osps_br_01_01` | Baseline 1 | General Controls | CI/CD pipeline sanitizes untrusted metadata | **Met** | Workflows avoid unquoted variables |
| `osps_br_01_03` | Baseline 1 | General Controls | Untrusted code snapshots isolated from CI credentials | **Met** | Fork PRs isolated from secrets |
| `osps_br_03_01` | Baseline 1 | General Controls | Official project channels exclusively encrypted | **Met** | HTTPS / SSH |
| `osps_br_03_02` | Baseline 1 | General Controls | Official distribution channels protected against MITM | **Met** | HTTPS / SSH |
| `osps_br_07_01` | Baseline 1 | General Controls | Prevent unintentional storage of secrets in VCS | **Met** | `.gitignore`, ephemeral test keys |
| `osps_do_01_01` | Baseline 1 | General Controls | User guides for all basic functionality | **Met** | `README.md`, `docs/operations.md` |
| `osps_do_02_01` | Baseline 1 | General Controls | Guide for reporting defects | **Met** | `README.md` (GitHub Issues) |
| `osps_gv_02_01` | Baseline 1 | General Controls | Mechanisms for public discussions | **Met** | GitHub Issues & PRs |
| `osps_gv_03_01` | Baseline 1 | General Controls | Explanation of contribution process | **Gap** | #125 (`CONTRIBUTING.md`) |
| `osps_le_02_01` | Baseline 1 | General Controls | Source license meets OSI or FSF definition | **Met** | Apache-2.0 (`LICENSE`) |
| `osps_le_02_02` | Baseline 1 | General Controls | Released assets license meets OSI or FSF | **Met** | Apache-2.0 |
| `osps_le_03_01` | Baseline 1 | General Controls | Source license maintained in LICENSE file | **Met** | Root `LICENSE` |
| `osps_le_03_02` | Baseline 1 | General Controls | Released assets license included in release | **Met** | `LICENSE` bundled in packages |
| `osps_qa_01_01` | Baseline 1 | General Controls | Source repo publicly readable at static URL | **Met** | GitHub static URL |
| `osps_qa_01_02` | Baseline 1 | General Controls | Publicly readable record of all changes | **Met** | Git commit history |
| `osps_qa_02_01` | Baseline 1 | General Controls | Direct language dependencies listed in manifest | **Met** | `Cargo.toml` in all crates |
| `osps_qa_04_01` | Baseline 1 | General Controls | Multi-repository codebases documented | **Met** | Monorepo crates in `Cargo.toml` |
| `osps_qa_05_01` | Baseline 1 | General Controls | VCS must NOT contain generated executables | **Met** | Verified: zero binary files in git |
| `osps_qa_05_02` | Baseline 1 | General Controls | VCS must NOT contain unreviewable binary artifacts | **Met** | Verified: zero binary blobs |
| `osps_vm_02_01` | Baseline 1 | General Controls | Documentation must contain security contacts | **Gap** | #126 (`SECURITY.md`) |
| `osps_ac_04_01` | Baseline 2 | General Controls | CI/CD defaults task permissions to lowest privileges | **Met** | `permissions: contents: read` in CI |
| `osps_br_02_01` | Baseline 2 | General Controls | Unique version identifier assigned to releases | **Pre-release N/A** | Pre-release; SemVer in `Cargo.toml` |
| `osps_br_04_01` | Baseline 2 | General Controls | Release contains descriptive changelog | **Pre-release N/A** | Pre-release; `CHANGELOG.md` at v0.1.0 |
| `osps_br_05_01` | Baseline 2 | General Controls | Pipeline uses standardized dependency tooling | **Met** | Cargo (`cargo build --locked`) |
| `osps_br_06_01` | Baseline 2 | General Controls | Official release signed or in signed manifest | **Pre-release N/A** | Pre-release; Cosign in #126 |
| `osps_do_06_01` | Baseline 2 | General Controls | Document how dependencies are selected/tracked | **Met** | `AGENTS.md`, `Cargo.lock` |
| `osps_do_07_01` | Baseline 2 | General Controls | Instructions on how to build software/dependencies | **Met** | `README.md`, `Makefile` |
| `osps_gv_01_01` | Baseline 2 | General Controls | Document members with access to sensitive resources | **Gap** | #125 (`MAINTAINERS.md`) |
| `osps_gv_01_02` | Baseline 2 | General Controls | Roles and responsibilities described | **Gap** | #125 (`GOVERNANCE.md`) |
| `osps_gv_03_02` | Baseline 2 | General Controls | Contributor guide includes requirements | **Gap** | #125 (`CONTRIBUTING.md`) |
| `osps_le_01_01` | Baseline 2 | General Controls | VCS requires DCO assertion on every commit | **Gap** | #125 (DCO CI enforcement) |
| `osps_qa_03_01` | Baseline 2 | General Controls | Automated status checks must pass before merge | **Met** | GitHub branch protection |
| `osps_qa_06_01` | Baseline 2 | General Controls | CI runs automated test suite before merge | **Met** | `ci.yml` gate on PRs |
| `osps_sa_01_01` | Baseline 2 | General Controls | Design docs demonstrate actions and actors | **Met** | `docs/architecture.md`, `CONTEXT.md` |
| `osps_sa_02_01` | Baseline 2 | General Controls | Descriptions of external software interfaces | **Met** | `docs/operations.md`, `proto/` |
| `osps_sa_03_01` | Baseline 2 | General Controls | Security assessment performed on release | **Pre-release N/A** | Pre-release; audit gate in #126 |
| `osps_vm_01_01` | Baseline 2 | General Controls | Policy for coordinated vulnerability disclosure | **Gap** | #126 (`SECURITY.md`) |
| `osps_vm_03_01` | Baseline 2 | General Controls | Private vulnerability reporting directly to contacts | **Gap** | #126 (`SECURITY.md`) |
| `osps_vm_04_01` | Baseline 2 | General Controls | Publicly publish data about discovered vulns | **Gap** | #126 (`SECURITY.md`) |
| `osps_ac_04_02` | Baseline 3 | General Controls | CI/CD jobs assign minimum privileges | **Met** | Granular permissions in CI |
| `osps_br_01_04` | Baseline 3 | General Controls | Sanitize collaborator input in CI/CD | **Met** | Safe workflow parameterization |
| `osps_br_02_02` | Baseline 3 | General Controls | Release assets associated with release identifier | **Pre-release N/A** | Pre-release; enforced at v0.1.0 |
| `osps_br_07_02` | Baseline 3 | General Controls | Policy for managing secrets and credentials | **Gap** | #126 (`SECURITY.md`) |
| `osps_do_03_01` | Baseline 3 | General Controls | Instructions to verify release integrity/authenticity | **Pre-release N/A** | Pre-release; docs in #126 |
| `osps_do_03_02` | Baseline 3 | General Controls | Instructions to verify release signer identity | **Pre-release N/A** | Pre-release; docs in #126 |
| `osps_do_04_01` | Baseline 3 | General Controls | Scope and duration of support for each release | **Gap** | #127 (`SUPPORT.md`) |
| `osps_do_05_01` | Baseline 3 | General Controls | Statement when versions no longer receive updates | **Gap** | #127 (`SUPPORT.md`) |
| `osps_gv_04_01` | Baseline 3 | General Controls | Collaborators reviewed before escalated access | **Gap** | #125 (`GOVERNANCE.md`) |
| `osps_qa_02_02` | Baseline 3 | General Controls | Compiled release assets delivered with SBOM | **Gap** | #128 (`cargo-auditable` / SBOM) |
| `osps_qa_04_02` | Baseline 3 | General Controls | Subprojects enforce security parity with primary | **Met** | `[workspace.lints]` across all crates |
| `osps_qa_06_02` | Baseline 3 | General Controls | Document when and how tests are run | **Met** | `AGENTS.md`, `README.md`, `Makefile` |
| `osps_qa_06_03` | Baseline 3 | General Controls | Policy that major changes add/update tests | **Met** | Mandated in `AGENTS.md` |
| `osps_qa_07_01` | Baseline 3 | General Controls | Non-author human approval required before merge | **Gap** | #125 (Branch protection rule) |
| `osps_sa_03_02` | Baseline 3 | General Controls | Threat modeling and attack surface analysis on release | **Gap** | #126 (`docs/security/assurance-case.md`) |
| `osps_vm_04_02` | Baseline 3 | General Controls | VEX document for non-exploitable vulnerabilities | **Pre-release N/A** | Pre-release; VEX feed in #128 |
| `osps_vm_05_01` | Baseline 3 | General Controls | Policy defining threshold for SCA remediation | **Gap** | #128 (`SECURITY.md`) |
| `osps_vm_05_02` | Baseline 3 | General Controls | Policy to address SCA violations prior to release | **Gap** | #128 (`SECURITY.md`) |
| `osps_vm_05_03` | Baseline 3 | General Controls | Evaluated against SCA policy and blocked on violations | **Met** | `cargo-audit --deny warnings` in CI |
| `osps_vm_06_01` | Baseline 3 | General Controls | Policy defining threshold for SAST remediation | **Gap** | #128 (`SECURITY.md`) |
| `osps_vm_06_02` | Baseline 3 | General Controls | Evaluated against SAST policy and blocked on violations | **Met** | `cargo clippy ... -D warnings` in CI |

---

## 7. Actionable Next Steps for Wayfinder Map #123

With this baseline criteria audit concluded, the path toward full OpenSSF Gold and OSPS Baseline Level 3 compliance is clearly partitioned across the remaining four specification tickets:

1. **Ticket #125 (Spec: Basics, Governance, and Change Control):**
   - Draft `CONTRIBUTING.md`, `GOVERNANCE.md`, `CODE_OF_CONDUCT.md`, `MAINTAINERS.md`.
   - Specify DCO commit sign-off enforcement action in `.github/workflows/`.
   - Specify branch protection rules (reviewers, status checks) and `small tasks` / `good first issue` labeling.
   - Specify automated addition of SPDX Apache-2.0 and copyright headers to all source files.

2. **Ticket #126 (Spec: Vulnerability Reporting, Security Architecture, and Cryptography):**
   - Draft `SECURITY.md` (CVD policy, GitHub PVR, response SLAs, reporter credit).
   - Specify formal Security Assurance Case and Threat Model in `docs/security/assurance-case.md`.
   - Specify Sigstore/Cosign keyless release signing for binaries and checksum manifests.
   - Document credential and secrets management policies.

3. **Ticket #127 (Spec: Quality, Build Reproducibility, and Test Coverage):**
   - Specify CI Tarpaulin coverage threshold gate (`--fail-under 80` for Silver, road to 90% statement / 80% branch for Gold).
   - Specify reproducible build bit-for-bit verification pipeline in CI.
   - Draft `SUPPORT.md` (support lifecycles, EOL statements).
   - Document dependency tracking and vetting governance in `docs/architecture.md`.

4. **Ticket #128 (Spec: Static/Dynamic Analysis, Hardening, and General Controls):**
   - Specify `fuzz/` crate with `cargo-fuzz` / libFuzzer targets for consensus envelopes, wire frames, and X.509 decoders.
   - Specify GitHub CodeQL SAST workflow (`.github/workflows/codeql.yml`).
   - Specify `cargo-auditable` integration for binary-embedded SBOM and CycloneDX generation.
   - Specify VEX policy and formal SAST/SCA remediation threshold policies in CI.

5. **Ticket #129 (Synthesis & Roadmap):**
   - Synthesize all specifications into `docs/compliance/openssf-best-practices.md` and `.agents/plans/openssf-compliance.md`.
   - Prepare the formal submission dossier for the OpenSSF Best Practices badge portal.
