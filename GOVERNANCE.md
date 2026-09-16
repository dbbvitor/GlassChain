# Governance

GlassChain is a FLOSS distributed ledger (Apache-2.0) for transparent
supply-chain transactions. This document defines how the project is run.

## Roles

### Maintainer

The maintainer has write access to the repository, approves and merges pull
requests, manages releases and security advisories, and enforces the
[Code of Conduct](CODE_OF_CONDUCT.md).

Current maintainer: **dbbvitor** (project lead).

### Contributor

Anyone submitting issues, pull requests, or documentation. Contributors retain
copyright and assert authorship rights via the
[DCO](https://developercertificate.org/) sign-off (see
[CONTRIBUTING.md](CONTRIBUTING.md)).

## Decision process

- **Everyday changes** (bug fixes, docs, tests): a contributor opens a pull
  request; any maintainer reviews and merges when CI passes.
- **Substantial changes** (new crates, protocol changes, public API, security
  model): discussed in a GitHub issue first; the maintainer records the
  rationale. Architecture decisions that are hard to reverse, surprising
  without context, or real trade-off picks are written up as ADRs in
  [`docs/adr/`](docs/adr/).
- **Disagreements**: resolved by discussion between the maintainer and the
  contributor. If no consensus is reached, the maintainer makes the final call
  and documents the reasoning on the issue.

## Continuity and bus factor

The project currently has a single maintainer. Bus factor of 2+ is an explicit
goal for the public release phase (see the
[roadmap](README.md#roadmap)); it is documented honestly as **not yet met**
rather than simulated. Until then, continuity is provided by:

- Full public commit history and CI configuration in the repository, so a
  new maintainer can reconstruct every decision.
- Architecture decisions recorded in `docs/adr/` and domain language in
  [`CONTEXT.md`](CONTEXT.md).
- Repository admin access held by the maintainer's GitHub account with 2FA
  (hardware-backed); an emergency recovery path exists through GitHub's
  account-recovery and ownership-transfer mechanisms to a named successor
  recorded in the project's private records.

The project also uses GitHub's [Private Vulnerability
Reporting](https://github.com/dbbvitor/GlassChain/security/advisories) — see
[SECURITY.md](SECURITY.md) — which remains available to reporters
independently of any single maintainer's availability.

## Adding maintainers

A contributor is invited to maintainership after sustained, high-quality
contributions (roughly: non-trivial merged work across at least three months).
Invitations are made publicly on a GitHub issue. Each additional maintainer
reduces the project's single point of failure and is recorded here.

## Small tasks

Issues suitable for new contributors are labeled `good first issue` or
`small task` in the [issue tracker](https://github.com/dbbvitor/GlassChain/issues).
