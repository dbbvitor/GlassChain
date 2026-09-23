# Mutation equivalents (do not chase)

Survivors from the #166 full-run cleanup that are **not testable by design**:
the mutation is unobservable through any public behavior, changes only log
text, or is unreachable in the reachable state space. Re-check this list when
the surrounding code changes; otherwise do not write tests for these.

## node.rs

Log-only — the mutated expression only decides whether a `log::warn!` fires:

- `load_equivocation_proofs` / `load_tofu_pins` `> 0` log gates.
- `handle_proposal`, `handle_precommit`, `process_message`: the
  `if !send(..) { warn }` guards.
- `process_message` `rejected = has_verifier && !org_verified`: only the
  rejection log wording changes.
- `process_message` private-payload `(true, false) if !verifier_configured`:
  both arms reject; only the reason string differs.
- `reconcile_private_payloads` `sent += 1`: `sent` appears only in the log.
- `process_message` `block.index > 0` in the future-timestamp guard: a
  genesis-index block cannot be appended either way.

Unreachable or behaviorally identical:

- `handle_peer` `while consensus_open || background_open`: neither channel's
  senders can all drop before `handle_peer` returns, so the loop exits at the
  same point either way.
- `collect_phase_votes` batch bound (`<` vs `==` / `>` / `<=`): the drain batch
  size changes by one; the collected set, per-voter dedup and the absolute
  deadline are unchanged.
- `handle_vote` `vote.height > 1` → `>=`: at height 1 `retire_below(0)` is a
  no-op, so both branches are identical.
- `process_message` `(false, block.index > expected)` → `>=`: the else branch
  already guarantees `index != expected`.
- `verify_chain_certificates` `block.index + 1` in the change-point push: the
  snapshot is pushed *after* its own block is verified, so a set only ever
  governs later blocks — for every block `h`, every change already processed
  (`c < h`) satisfies `c + 1 <= h`, `c - 1 <= h` and `c <= h` alike. The
  effective set selected is identical under all three variants.
- `insecure_tls_allowed`: cargo-mutants runs `--all-features`, so the
  `insecure-tls` feature is compiled in and the function returns `true`
  regardless of the env var.

## identity / sdk

- `GlasschainClient::build_asset_registration_tx` `if !missing.is_empty()`
  guard: log-only.
- `cert_verifier::verified_subject_cn` `PrintableString` arm: rcgen emits only
  UTF8String CNs, so no fixture reaches the arm through the public API.
