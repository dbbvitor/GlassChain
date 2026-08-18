# CLAUDE.md

**All project instructions live in [`AGENTS.md`](AGENTS.md).** This file only
imports them so Claude Code picks them up automatically. Do not duplicate rules
here — if something needs to change, change `AGENTS.md`.

@AGENTS.md

---

## Quick reference

```bash
cargo check --workspace --all-targets   # fast feedback loop
cargo test --workspace                  # must pass before you finish
cargo clippy --workspace --all-targets  # must not add new warnings
```

## Claude-specific working notes

- **Verify your work.** `cargo test --workspace` is the check to run and iterate
  against — it is fast and currently fully green. Do not declare a task done
  without showing its output.
- **Never run `glasschain-node` without a timeout.** It starts an interactive
  REPL that blocks on stdin and will hang the session. Use the integration tests
  in `crates/glasschain-network/tests/` instead.
- **Plan before multi-file work.** Write the plan to `.agents/plans/<slug>.md`
  and implement against it. See the `.agents/` section of `AGENTS.md`.
- **Use subagents for exploration.** This is a ~16k-line Rust workspace across 11
  crates; delegate broad code searches so they don't consume the main context window.
- **Don't run `cargo fmt --all`.** The repo has a pre-existing formatting backlog
  and it will produce an unreviewable diff. Format only the files you touched.
- **Record durable findings** in `.agents/memories/<topic>.md`, and promote
  anything that is really a project rule into `AGENTS.md`.
