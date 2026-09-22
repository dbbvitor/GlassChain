# ────────────────────────────────────────────────────────────────────────────
# GlassChain — Makefile
#
# Setup, build, test, and gate targets for the GlassChain Rust workspace.
#
# The toolchain is pinned to 1.98.1 in `rust-toolchain.toml`; cargo/rustup pick
# it up automatically. `make ci` is the fast local gate; `make check` runs
# formatting, lints, and a type-check without tests.
#
# Gate targets (mirror CI):
#   check       fmt-check + clippy + cargo check          (fast, no tests)
#   test        cargo nextest run --profile ci            (same flags as CI)
#   analysis    cargo deny check + cargo machete          (supply chain + deps)
#   ci          check -> test -> analysis
#
# Deep tools are opt-in (nightly or long runtimes):
#   careful  mutants  mutants-diff  miri  sanitize  kani  verus  snarf  llvm-lines
#
# `make tools` installs the stable tooling; `make tools-nightly` adds nightly
# components and nightly-only tools; `make tools-formal` installs Kani and
# points at the pinned Verus release.
#
# Notes:
#   * protoc is required to build glasschain-rpc (tonic_prost_build). `make
#     setup` installs it via the system package manager (brew/apt/dnf/pacman)
#     and may prompt for sudo.
#   * Never run `make node` from an automated step: it starts an interactive
#     REPL that blocks on stdin.
#   * This Makefile targets macOS and Linux. On Windows use the CI workflow or
#     a GNU-make shell (e.g. Git Bash, WSL).
# ────────────────────────────────────────────────────────────────────────────

SHELL := /bin/sh

# Flags shared by the gate targets (override: make test CARGO_FLAGS="...").
CARGO_FLAGS := --workspace --all-targets --all-features --locked

# Test targets drop --all-targets: criterion bench *executions* dominate the
# run (their test-mode setup mines a 10k-block history in debug) and yield no
# test coverage; they stay compile-checked via check/clippy and run for real
# through `make bench`. Mirrors the CI test job.
TEST_FLAGS := --workspace --lib --bins --tests --all-features --locked

# The network integration tests allocate loopback ports through the shared
# per-process band allocator (glasschain-network/tests/common/ports.rs), so
# nextest's process-per-test isolation runs them in parallel safely.

# Pinned channel from rust-toolchain.toml.
TOOLCHAIN := 1.98.1

# Host triple for sanitizer builds (`--target` keeps rustflags off build scripts).
HOST := $(shell rustc -vV 2>/dev/null | sed -n 's/^host: //p')

# `make mutants` scope; the CI full run shards all 12 crates.
MUTANTS_PKG ?= glasschain-core

# Miri: six-crate allowlist, strict flags, no leak exemption (ticket #153).
MIRI_FLAGS := -Zmiri-disable-isolation -Zmiri-strict-provenance -Zmiri-symbolic-alignment-check
MIRI_PKGS := -p glasschain-core -p glasschain-contracts -p glasschain-indexer \
             -p glasschain-workflows -p glasschain-storage -p glasschain-sdk
MIRI_SKIPS := --skip wasm --skip sled_backend \
              --skip test_pending_pool_bound_rejects_and_drains \
              --skip test_slice_quota_spreads_a_burst_across_rounds \
              --skip ledger_default_uses_the_workspace_difficulty \
              --skip test_d3_index_semantics_match_full_rebuild_after_history_growth

.DEFAULT_GOAL := help

.PHONY: help setup tools tools-nightly tools-formal build build-release check \
        test test-pkg test-one analysis fmt fmt-check clippy snarf careful \
        mutants mutants-diff miri sanitize kani verus llvm-lines bench audit \
        coverage coverage-xml ci doc node clean

help: ## Show this help
	@awk -F ':.*## ' '/^[a-zA-Z0-9_-]+:.*## / {printf "  %-16s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

## ── Setup ──────────────────────────────────────────────────────────────────

setup: ## Install the pinned toolchain, rustfmt/clippy, and protoc (may need sudo)
	@rustup toolchain install $(TOOLCHAIN)
	@rustup component add --toolchain $(TOOLCHAIN) rustfmt clippy
	@echo "==> Ensuring protoc is available (required to build glasschain-rpc)"
	@if command -v protoc >/dev/null 2>&1; then \
	  protoc --version; \
	else \
	  case "$$(uname -s)" in \
	    Darwin) brew install protobuf ;; \
	    Linux) \
	      if command -v apt-get >/dev/null 2>&1; then sudo apt-get update && sudo apt-get install -y protobuf-compiler; \
	      elif command -v dnf >/dev/null 2>&1; then sudo dnf install -y protobuf-compiler; \
	      elif command -v pacman >/dev/null 2>&1; then sudo pacman -S --noconfirm protobuf; \
	      else echo "!! Install protobuf-compiler with your distro's package manager"; fi ;; \
	    *) echo "!! Install protoc manually"; exit 1 ;; \
	  esac; \
	fi

tools: ## Install the stable tooling used by the gate and deep targets
	cargo install --locked cargo-nextest
	cargo install --locked cargo-machete
	cargo install --locked cargo-deny
	cargo install --locked cargo-mutants
	cargo install --locked typos-cli
	cargo install --locked cargo-hack
	cargo install --locked cargo-llvm-cov
	cargo install --locked cargo-tarpaulin
	cargo install --locked cargo-audit

tools-nightly: ## Install nightly components and nightly-only tools (miri, careful, snarf)
	rustup toolchain install nightly
	rustup component add --toolchain nightly rust-src miri
	cargo install --locked cargo-careful
	cargo install --locked cargo-snarf

tools-formal: ## Install Kani; print the pinned Verus release pointer
	cargo install --locked kani-verifier
	cargo kani setup
	@echo "Verus has no aarch64 Linux artifact: download the pinned release zip"
	@echo "(x86_64 Linux/macOS) from https://github.com/verus-lang/verus/releases"
	@echo "and put 'verus' + 'cargo-verus' on PATH."

## ── Build ──────────────────────────────────────────────────────────────────

build: ## Build the workspace (debug)
	cargo build

build-release: ## Build the workspace (release)
	cargo build --release

## ── Gate ───────────────────────────────────────────────────────────────────

check: ## Fast gate: formatting, lints, and type-check (no tests)
	$(MAKE) fmt-check
	$(MAKE) clippy
	cargo check $(CARGO_FLAGS)

test: ## Run the full workspace suite with nextest (same flags as CI)
	cargo nextest run --profile ci $(TEST_FLAGS)

test-pkg: ## Test one crate: make test-pkg pkg=glasschain-network
	cargo nextest run -p $(pkg) --profile ci $(TEST_FLAGS)

test-one: ## Run tests matching a substring: make test-one pkg=glasschain-core test=mine
	cargo nextest run $(if $(pkg),-p $(pkg),) $(test)

analysis: ## Supply-chain and dependency hygiene (CI gate)
	cargo deny --all-features check
	# `crates` scopes machete to the workspace: `--with-metadata` rewrites the
	# lockfile of every package it scans, and the excluded demo/ has its own.
	cargo machete --with-metadata crates

## ── Lint & quality ──────────────────────────────────────────────────────────

fmt: ## Format the entire workspace (writes changes; prefer scoping to touched files)
	cargo fmt --all

fmt-check: ## Verify formatting without modifying files (CI gate)
	cargo fmt --all --check

clippy: ## Run clippy with warnings as errors (CI gate)
	cargo clippy $(CARGO_FLAGS) -- -D warnings

## ── Deep tools (opt-in) ─────────────────────────────────────────────────────

snarf: ## Cache-line false-sharing check (nightly; run `make tools-nightly` first)
	cargo +nightly snarf --format github --color never

careful: ## Run the suite under cargo-careful (nightly, std debug assertions)
	cargo +nightly careful nextest run --profile ci $(TEST_FLAGS)

mutants: ## Mutation-test one crate (default glasschain-core; override MUTANTS_PKG=...)
	cargo mutants -p $(MUTANTS_PKG) --timeout 60

mutants-diff: ## Mutation-test only the current diff (the CI PR-gate command)
	# --timeout is a CLI-only option: `.cargo/mutants.toml` rejects it.
	cargo mutants --in-diff --baseline=skip --in-place --timeout 60

miri: ## Run Miri over the six-crate allowlist with the strictest flags
	MIRIFLAGS="$(MIRI_FLAGS)" cargo +nightly miri test $(MIRI_PKGS) --lib -- $(MIRI_SKIPS)

sanitize: ## Run the suite under ASan/LSan (nightly; leaks are failures)
	RUSTFLAGS="-Zsanitizer=address" ASAN_OPTIONS=detect_leaks=1 \
	  cargo +nightly test --workspace --lib --bins --tests --all-features --locked \
	  -Zbuild-std --target $(HOST)

kani: ## Run the Kani proofs for glasschain-core
	cargo kani -p glasschain-core --harness proofs::iso8601_check_is_total_and_bounded --default-unwind 16

verus: ## Verify the critical-code roadmap (starts with glasschain-vm gas)
	cargo verus verify -p glasschain-vm

llvm-lines: ## Compile-time bloat diagnostic: LLVM IR lines per generic function
	cargo llvm-lines -p glasschain-core | head -30

bench: ## Run the criterion benches
	cargo bench -p glasschain-core
	cargo bench -p glasschain-vm
	cargo bench -p glasschain-workflows

audit: ## Audit dependencies for known vulnerabilities
	cargo audit --deny warnings --file Cargo.lock

coverage: ## HTML coverage report (cargo-llvm-cov; CI switches to this engine next)
	cargo llvm-cov nextest --html --profile ci $(TEST_FLAGS)

coverage-xml: ## Cobertura XML coverage with the 90% line gate
	cargo llvm-cov nextest --cobertura --output-path cobertura.xml \
	  --fail-under-lines 90 --profile ci $(TEST_FLAGS)

## ── Aggregate ───────────────────────────────────────────────────────────────

ci: ## Fast local gate: check -> test -> analysis
	$(MAKE) check
	$(MAKE) test
	$(MAKE) analysis

## ── Run & docs ──────────────────────────────────────────────────────────────

node: ## Run a node REPL: make node id=node-1 port=8000 (interactive)
	@test -n "$(id)" || (echo "usage: make node id=node-1 port=8000"; exit 1)
	@test -n "$(port)" || (echo "usage: make node id=node-1 port=8000"; exit 1)
	cargo run --release -p glasschain-node -- --id $(id) --listen 0.0.0.0:$(port)

doc: ## Build the crate documentation
	cargo doc --workspace --no-deps

clean: ## Remove build artifacts (target/)
	cargo clean
