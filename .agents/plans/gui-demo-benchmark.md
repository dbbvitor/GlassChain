# Plan — `glasschain-demo`: a visual demo and benchmark harness (gpui)

**Status:** draft — not started
**Date:** 2026-09-02
**Issue:** #61

## Goal

A desktop application a human can run to *watch* GlassChain work: synthetic
supply-chain traffic flowing through a live multi-node federation, with
throughput visible as it happens and a lot traceable from shelf back to origin
in a few clicks. When this is done, demonstrating GlassChain to a
non-Rust-reading audience takes one command instead of a REPL walkthrough, and
the throughput numbers in `docs/benchmarks/` have a visual counterpart driven by
the same node code.

Two audiences, one binary:

- **Demonstrate** — a regulator, a pharma ops lead, or a reviewer sees custody
  transfers, endorsements, private-payload exchange, and a recall propagating
  across orgs in real time.
- **Benchmark** — an engineer sees committed tx/s, block interval, block size,
  quorum-certificate size, and mempool depth as live plots, against the same
  compact workload `consensus_capacity.rs` already defines.

## Context

- Nodes are `glasschain_network::Node`, driven with Tokio. Everything the GUI
  needs is already public: `ledger_snapshot()`, the event bus
  (`glasschain-indexer`), the provenance/lineage queries behind #39, and the
  flow runners in `glasschain-workflows`.
- `crates/glasschain-network/tests/consensus_capacity.rs` already builds a star
  topology and a compact ADR-010 §7 workload, and already measures latency,
  block size, certificate size, pool depth, and fan-out. **The demo should drive
  the same workload generator, not invent a second one** — extract it rather
  than duplicate it.
- `docs/benchmarks/consensus-capacity.md` is the existing text-form output. The
  GUI is a second renderer of the same measurements, not a new source of truth.

## Approach

### Framework: gpui, pinned to crates.io `=0.2.2`

Chosen because it is the framework the user asked for, it is Rust-native, and
it is GPU-accelerated enough to animate a few hundred moving elements without
becoming the bottleneck in a throughput demo.

Verified 2026-09-02:

- `gpui` **is** on crates.io — current stable **0.2.2** (2025-10-22, Apache-2.0,
  edition 2024). The old "git dependency only" advice is out of date.
- **Pin `=0.2.2` exactly.** It is pre-1.0 and the README promises breaking
  changes between versions. zed `main` has already split the bootstrap into a
  separate `gpui_platform` crate which **is not published** — so consuming
  `main` from git would drag in unpublished companion crates at a pinned rev.
  Treat git `main` as radioactive.
- Zed pins Rust **1.97.1**; `gpui` 0.2.2 declares no `rust-version` but is
  edition 2024, so it needs ≥1.85. **Our workspace pins 1.95 — this is
  unverified and is step 0 below.**
- Linux runtime needs `libvulkan1` plus a working Wayland or X11 display;
  fontconfig/xkbcommon/wayland/Vulkan are `dlopen`'d, so *compiling* needs
  little beyond a C toolchain.
- No built-in charting. Plots are hand-drawn on the `canvas` element:
  `canvas(prepaint: FnOnce(Bounds<Pixels>, &mut Window, &mut App) -> T, paint: FnOnce(Bounds<Pixels>, T, &mut Window, &mut App))`,
  drawing with `window.paint_path(...)`.

### Isolation: excluded from the workspace

**This is the load-bearing decision.** `cargo check/test/clippy --workspace
--all-targets --all-features --locked` is a mandatory CI gate on Ubuntu, macOS,
and Windows. A cargo feature does **not** dodge `--all-features`, so gating the
GUI behind a feature would put blade, cosmic-text, resvg, accesskit, smol, and
the `windows` crate into every gate on every runner, and would let pre-1.0 gpui
churn break the whole workspace.

So: `demo/` lives in the repo but is **excluded from the workspace** via
`[workspace] exclude = ["demo"]` in the root `Cargo.toml`. It gets its own
`Cargo.lock` and its own CI job. It depends on the workspace crates by path.
The four mandatory gates stay exactly as fast and as green as they are today.

Rejected alternative — GUI as a 13th workspace member: fails the CI-cost and
churn test above.
Rejected alternative — a separate repository: loses path dependencies, so the
demo would silently rot against `main`.

### Async: Tokio on its own threads, channel into gpui

gpui runs its own **smol** executor and there is **no documented Tokio bridge**.
Do not try to run the node inside gpui's executor.

Pattern: the federation runs on a normal Tokio runtime on its own threads and
publishes snapshots/events into a channel. The UI drains that channel inside a
gpui task:

```rust
cx.spawn(async move |this, cx| {
    while let Ok(msg) = rx.recv().await {
        let _ = this.update(cx, |this, cx| { this.apply(msg); cx.notify(); });
    }
}).detach();
```

Send **snapshots at a fixed cadence** (~30 Hz), not one message per transaction
— at target throughput a per-tx channel would make the UI the bottleneck and the
benchmark would measure the renderer.

### Synthetic data

A generator over the real canonical schema v1 families — no bypass of
validation, endorsement, or the PDC boundary. If the demo can produce a record
the node would reject, the demo is lying.

- A plausible pharma federation: manufacturer → distributor → logistics →
  pharmacy, plus a regulator with default collection visibility.
- Deterministic from a seed, so a demo run is reproducible and a benchmark run
  is comparable.
- Realistic GTINs, lot numbers, and Anvisa registration shapes; currency always
  integer minor units.
- Scenario switches: steady-state replenishment, demand spike, a recall
  cascade, a disputed shipment, and a node partition.

## Steps

- [ ] **0. Toolchain spike (blocking, ~15 min).** `cargo add gpui@=0.2.2` in a
      scratch crate on our pinned 1.95 and build `hello_world`. If 1.95 cannot
      compile edition-2024 gpui, stop and decide: bump the workspace pin, or pin
      the toolchain for `demo/` only. **Do not start anything below until this
      is answered.**
- [ ] 1. `[workspace] exclude = ["demo"]` in the root `Cargo.toml`; scaffold
      `demo/` with `gpui = "=0.2.2"` and path deps on the workspace crates.
- [ ] 2. Extract the compact workload generator out of
      `crates/glasschain-network/tests/consensus_capacity.rs` into something
      both the test and the demo call. The test must keep passing unchanged.
- [ ] 3. Headless federation driver: N in-process nodes on Tokio, snapshot
      publisher at a fixed cadence, scenario switches. **Testable without a
      display** — this is where the logic tests live.
- [ ] 4. gpui shell: window, layout, scenario controls, start/stop/seed.
- [ ] 5. Network view — nodes, peer links, blocks committing, transactions
      animating along custody edges. Public records and PDC commitments must be
      visually distinct; showing a private payload the viewing org may not read
      would misrepresent the privacy model.
- [ ] 6. Throughput panel on `canvas` — committed tx/s, block interval, block
      size, QC size, mempool depth. Same metrics as
      `docs/benchmarks/consensus-capacity.md`.
- [ ] 7. Traceability view — pick a lot, walk its provenance back to origin via
      the #39 lineage queries, showing which hops are public and which are
      commitments the viewer cannot open.
- [ ] 8. Separate CI job (Linux only to start): `cargo check` + `cargo clippy -D
      warnings` + headless tests in `demo/`. Never added to the four mandatory
      workspace gates.
- [ ] 9. `docs/demo.md` — what it shows, how to run it, what is synthetic, and
      an explicit statement that its numbers are **not** the ADR-010 adoption-gate
      benchmark.

## Validation

- The four workspace gates must be **byte-for-byte unaffected**: confirm
  `cargo check --workspace --all-targets --all-features --locked` does not
  compile a single gpui dependency.
- `cd demo && cargo clippy --all-targets -- -D warnings` green.
- Headless tests cover the federation driver and the synthetic generator; UI
  rendering is not unit-tested (needs a display) and any smoke test that opens a
  window is `#[ignore]`d.
- `cargo test -p glasschain-network --test consensus_capacity` still passes
  after step 2.

## Out of scope

- **Not** the ADR-010 adoption-gate benchmark. That gate is a real 200/300-validator
  compact-workload testnet (#48, `consensus_capacity.rs`, `docs/benchmarks/`). The
  demo visualizes a small in-process federation and must never be cited as
  evidence for it. Say so in the UI.
- No web/WASM build, no remote-node attach, no persistence of demo runs.
- No new consensus, schema, or protocol behaviour. If the demo needs a
  capability the node lacks, that is a node ticket, not a demo ticket.
- Not a production observability surface. Real metrics/tracing remain their own
  unstarted work item in `requirements-alignment.md`.

## Risks

| Risk | Mitigation |
|---|---|
| gpui 0.2.2 will not build on Rust 1.95 | Step 0 spike, before anything else |
| Pre-1.0 API churn breaks the demo | Pinned `=0.2.2`, separate `Cargo.lock`, excluded from workspace gates |
| Headless CI cannot run GUI tests | Logic lives in the driver (step 3) and is tested without a display; window tests `#[ignore]`d |
| Demo numbers get quoted as benchmark evidence | Disclaimer in the UI *and* in `docs/demo.md`; keep `docs/benchmarks/` authoritative |
| Rendering becomes the throughput bottleneck | Fixed-cadence snapshots, not per-transaction messages |
