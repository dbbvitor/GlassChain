# Security Policy

GlassChain is a pre-release distributed ledger. **Do not use it with real
assets or data before reading the security expectations in
[README.md](README.md) ("Read this before trusting it with real data") and
[docs/threat-model.md](docs/threat-model.md).**

## Reporting a vulnerability

**Do not open a public issue for a security vulnerability.**

Report privately through **GitHub Private Vulnerability Reporting**:

> https://github.com/dbbvitor/GlassChain/security/advisories/new

This channel is encrypted end-to-end between you and the maintainer, requires
no account contact details beyond GitHub, and lets you follow the fix in the
same thread.

Please include:

- A description of the vulnerability and the affected component (crate,
  module, protocol path).
- Steps to reproduce or a proof of concept.
- Your assessment of impact, if you have one.

## Coordinated Vulnerability Disclosure (CVD)

GlassChain follows coordinated disclosure:

1. **Report received.** You will get an **initial response within 14 days**
   acknowledging the report and a triage outcome (accepted, needs info, or
   not a vulnerability).
2. **Fix development.** Accepted reports get a fix developed and validated
   privately; we aim to ship a release with the fix **within 60 days** of
   acceptance. Complex issues may take longer — we will keep you informed of
   the timeline in the advisory thread.
3. **Publication.** Once the fix ships, we publish a GitHub Security Advisory
   (GHSA). Credit goes to the reporter (unless you prefer anonymity); CVE
   identifiers are requested via GitHub when warranted.
4. **Credit.** Reporters of vulnerabilities fixed within the last 12 months
   are credited in the published advisory.

## Supported versions

GlassChain is **pre-release** (`0.1.0`, no tagged public releases yet).
Security fixes target the `main` branch; users are expected to build from a
recent commit. Versioned support branches will be defined at the first public
release.

## VEX statements

For dependencies where automated analysis (cargo-audit, Dependabot, CodeQL)
reports an issue we assess as a false positive or not exploitable in
GlassChain, the assessment is recorded as a **VEX statement** inside the
published GitHub Security Advisory for the dependency, using the advisory's
analysis fields (`not_affected`, `fixed`, `under_investigation`). Check the
project's security advisories for current VEX positions before reopening
flagged findings.

## Security-relevant invariants

The codebase enforces invariants documented in [AGENTS.md](AGENTS.md): TLS is
mandatory by default, certificate revocation is fail-closed (CRL + OCSP
staples), no environment-variable kill switches for security controls, no
`unsafe` code, and identity material never enters the storage seam. A change
weakening any of these is treated as a security defect.
