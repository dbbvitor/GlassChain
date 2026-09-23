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

## storage / rpc / cli

- `transient::TransientStore::record_expiry -> ()`: the expiry index is only a
  fast path. `purge_expired` falls back to enumerating persisted envelopes
  ("durable discovery"), so the expired set — and the returned count — is
  identical with an empty index. The index also cannot be observed directly.
- `transient::TransientStore::put` `now + retention_secs` vs `now * retention_secs`:
  the two agree for every retention except `1`, where `+` yields a live entry
  for the remainder of the current second and `*` expires it immediately. No
  clock injection exists, so the difference is only observable inside a
  sub-second window at `retention_secs == 1`.
- `MspAuthInterceptor::verify_request` / `AdminGate::authorize`
  `skew > N` vs `skew >= N`: the two differ only when the timestamp is exactly
  `N` seconds from `now`. The header is built and verified at different wall
  times, so neither direction is deterministic without clock injection.
- `glasschain-cli` `main -> Ok(())`: `main` is the binary entry point; the unit
  tests exercise parsing, not the process. Not reachable from a unit test.
- `channel_admin::connect_with_retry` `Instant::now() < deadline` vs `<=`: the
  retry loop's one-instant boundary is unobservable without injecting time.

## glasschain-node REPL (main.rs)

- `parse_price` `s.is_empty() || s.starts_with('-')`: the `||`/`&&` variants
  agree on every input — an empty string fails the later `whole.is_empty()`
  check, and a `-` prefix fails the all-digits check, so both branches return
  `None`. Confirmed by the existing `parse_price_rejects_*` tests.
- `parse_price` `!frac…all(is_ascii_digit) || frac.len() > 2`: same — a
  non-digit fraction fails `frac.parse()`, and a >2-digit fraction is caught by
  the `_ => return None` arm; both spellings return `None`.
- `parse_args` second `i += 1` vs `i *= 1`: only shifts how many loop steps run;
  the parsed `CliArgs` is unchanged for every flag/unknown/trailing-input shape
  the tests exercise.
- `usage -> ()`, `log_event -> ()`: side-effect-only (stderr / `log`); not
  observable through a unit test.
- `main -> ()`, its match-arm deletions and header guards: `main` is the binary
  entry point and is not invoked by the unit tests.
