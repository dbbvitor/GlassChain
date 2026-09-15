# Plan — Zero-trust residual work (post-Frontier B)

**Status:** shipped 2026-09-14 (ZT-R1–R4 implemented; retire this plan once
the PR carries it). Parked decisions unchanged.
**Related:** [zero-trust](zero-trust.md) §1–§8 (shipped items live there).
**Related:** [ADR-011](../../docs/adr/adr-011-federation-trust-store.md) ·
[ADR-013](../../docs/adr/adr-013-certificate-revocation.md) ·
[ADR-017](../../docs/adr/adr-017-deployment-trust-and-retention.md) ·
[deferred-code-debt](deferred-code-debt.md).

## Goal

Close the last open zero-trust threads: make an identity-backed node
restart-safe, complete the operator loop, and decide the durable evidence
question. Everything else in the zero-trust plan is shipped or
deferred-by-decision (see "Parked", below).

## Work items, in priority order

### ZT-R1 — Persisted node identity / key custody — **shipped (2026-09-14, ADR-018)**

`--identity-file <PATH>`: `Organization::export_json` / `import_json` carry
the Root CA key pair, serial bookkeeping, revocation history and member
seeds in an operator-owned file (mode `0600` on Unix, created on first
start, loaded after). Identity material deliberately stays **outside** the
storage seam (private keys must not leak into archived storage copies).
Restart re-presents the same key/cert/Root CA; a reconnecting node matches
its pin as `Known` (same fingerprint, same rotation key). Round-trip,
malformed-input and re-present tests in `glasschain-identity`.

**Problem.** `glasschain-node` creates a fresh `Organization` and a fresh
member identity **on every start** (`Organization::new(org)` in `main.rs`).
Consequences: a new ed25519 identity key, a new member certificate (new
serial), a new root CA, and a new TLS certificate every restart. Any peer
holding a persisted TOFU pin (`tofu:peer:<addr>`, #88) then sees an unknown
fingerprint **and** a pin whose rotation key is gone — reconnect fails until
an operator removes the pin by hand. Same for trust-store anchors holding a
peer's ephemeral root. This is the gap zero-trust §5 calls "persisted node
identity/key custody — related but not identical" to pin persistence.

**Do.**
1. Persist the node's identity key material through the storage seam
   (keyed like `tofu:peer:*`, e.g. `identity:seed:<node-id>`), generated on
   first start and reused after.
2. Persist the org's Root CA material alongside (the CA must outlive
   restarts for CRL minting, `ocsp_response_der`, and verification by peers
   to stay coherent), or load it from `--trust-store`-style files the
   operator owns.
3. Restart must re-present the **same** possession/rotation keys: a
   reconnecting identity-backed node signs its new TLS fingerprint with the
   pinned key and re-pins without operator intervention.
4. Never log or serialize the private key (AGENTS.md security rules);
   storage copies are scrubbed by `backup-scrub` semantics — audit what the
   key file needs (operator-owned file vs. storage state) before choosing.

**Acceptance.** A test that: (a) starts an identity-backed node with
persistent storage, (b) restarts it against the same storage, (c) reconnects
to a peer holding the persisted pin **without** operator intervention, and
(d) asserts the pin rotated via a signed rotation proof (#88 path) — plus a
fail case: a *different* node id claiming the same key is rejected.

### ZT-R2 — Admin CLI client — **shipped (2026-09-14)**

`glasschain channel-admin --endpoint --cert <PEM> --key-seed <HEX>
create-channel|add-member|remove-member …`. `glasschain_rpc::admin_headers_from_cert`
derives the node id from the certificate's subject CN and signs the header
challenge (the client side of `AdminGate`). Test: `channel_admin.rs` drives a
live `GlasschainServer` end to end — create/add accepted, a plain member
certificate refused with the `admin role` denial.

### ZT-R3 — Trust-store/CRL hot-reload — **shipped (2026-09-14)**

`build_trust_store_verifier` extracted in `glasschain-node`; the
`reload-trust-store <PATH>` REPL command re-runs it and swaps the verifier
atomically via `Node::set_cert_verifier` — no timer, refresh stays
operator-signalled. `AdminGate` holds its verifier behind a shared lock
(`update_verifier`), so a gate clone inside the gRPC server sees the swap.
Test: the loader loads dir/file stores and reports counts; peers
re-authorize at the next Hello (existing per-Hello tests).

Re-read `--trust-store` files on operator signal (signal or REPL command —
not a timer; refresh is deliberately out-of-band) and swap the verifier
atomically via `Node::set_cert_verifier`. Established-session reauthorization
is already per-Hello, so a rotated CRL takes effect at the next Hello.
Acceptance: a test rotates a CRL (revokes a member), reloads, and the next
Hello from that member fails org verification while identity pins survive.

### ZT-R4 — Durable equivocation evidence — **shipped (2026-09-14, shape (a))**

Proofs persist through the state seam (`equivocation:<height>:<round>:<key>`)
at detection and reload at startup — evidence outlives the session while
staying **off-chain**: no committed history, no replay impact, no automatic
exclusion (ADR-009). The accepted-limitation comment on the journal is
resolved. Corrupt persisted entries are logged and skipped (evidence is
advisory governance input, not a trust decision). Test:
`equivocation_proofs_survive_a_restart` (detect → persist → clear → reload →
proof still verifies).

## Parked (decision, not tasks — do not re-open without an explicit ask)

- **#74 on-chain revocation registry** and the **chain-derived MSP
  registry** (adjacent): ADR-017 keeps them deferred; D4 height bounds +
  CRLs + staples cover revocation until a production testnet argues
  otherwise.
- **Delegated OCSP responders**: rejected by ADR-017 (no responder egress).
  Do not re-open.
- **Remote-principal wiring for the endorsement provider** (`register_certificate`
  exists; no node wiring) — park until a deployment actually needs remote
  principal registration.
- **PQ archive evidence / migration policy**: owned by
  [post-quantum.md](post-quantum.md), not here.

## Non-goals

No env-var kill switches, no production bypasses, no responder network
queries, no automatic exclusion from equivocation evidence. Every
implementation runs the workspace gates and adds its named failure-case
test (AGENTS.md rules).
