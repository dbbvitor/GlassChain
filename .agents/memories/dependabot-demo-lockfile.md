# Dependabot and the standalone `demo/` lockfile

**Finding (2026-09-20).** Dependabot security-update jobs for `/demo` fail with
`failed to find a workspace root` and never open a PR. The demo is deliberately
`exclude`d from the root workspace (`demo/Cargo.toml` comment, plan §2) and
path-depends on `crates/{core,identity,indexer,network}`. Those crates declare
`[lints] workspace = true`, so cargo cannot parse them without the repo-root
`Cargo.toml` — and Dependabot does not fetch that root.

**Root cause is upstream.** Dependabot's cargo `file_fetcher.rb`
`workspace_member?` walks nested tables but **returns on the first hash value**
it meets. In these manifests the first table is `[package]`, which has no
`workspace = true`, so the `[lints] workspace = true` table is never reached and
the workspace root is not fetched. (The 2024 fix in dependabot-core #10550/#10629
only covers `[package]`-level inheritance such as `version.workspace = true`.)

**Workaround that would make it resolvable:** give the path-dep crates a
package-level workspace inheritance (`[workspace.package]` in the root plus e.g.
`edition.workspace = true` in each crate), so `workspace_member?` returns true
before hitting the early return. Not done — it touches all 12 crates and was not
worth it once the advisories were cleared.

**What actually resolved it:** bumping `libp2p` 0.56 → 0.57, which moves
`libp2p-mdns` to 0.49 (hickory-proto 0.26.3) and `libp2p-yamux` to 0.48
(yamux 0.14), clearing all three `demo/Cargo.lock` advisories in both lockfiles.
If future advisories land in the demo, this limitation returns — expect the
security-update job to fail until the manifests are restructured.
