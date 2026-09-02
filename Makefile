# ────────────────────────────────────────────────────────────────────────────
# GlassChain — Makefile
#
# Setup, build, test, and CI-gate targets for the GlassChain Rust workspace.
#
# The toolchain is pinned to 1.95 in `rust-toolchain.toml`; cargo/rustup pick it
# up automatically, so plain `cargo` commands here use the right toolchain.
#
# Targets mirror the gates in .github/workflows/ci.yml, so `make ci` locally
# reproduces what CI runs. `make setup` gets you from a fresh clone to a
# building workspace; `make test` runs the full workspace test suite.
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

# Flags shared by the CI-gate targets (override: make test CARGO_FLAGS="...").
CARGO_FLAGS := --workspace --all-targets --all-features --locked

# The network integration tests bind real loopback ports and do real TLS
# handshakes; serialised libtest execution avoids cross-test port races (CI
# uses the same setting). Override: make test RUST_TEST_THREADS=4
RUST_TEST_THREADS ?= 1

# Pinned channel from rust-toolchain.toml.
TOOLCHAIN := 1.95

.DEFAULT_GOAL := help

.PHONY: help setup tools build build-release check test test-pkg test-one \
        fmt fmt-check clippy bench audit coverage coverage-xml ci doc node clean

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

tools: ## Install optional tooling for the coverage and audit targets
	cargo install --locked cargo-tarpaulin
	cargo install --locked cargo-audit

## ── Build ──────────────────────────────────────────────────────────────────

build: ## Build the workspace (debug)
	cargo build

build-release: ## Build the workspace (release)
	cargo build --release

## ── Test ───────────────────────────────────────────────────────────────────

check: ## Type-check all targets (fast; run often while iterating)
	cargo check $(CARGO_FLAGS)

test: ## Run the full workspace test suite (CI gate)
	RUST_TEST_THREADS=$(RUST_TEST_THREADS) cargo test $(CARGO_FLAGS)

test-pkg: ## Test a single crate: make test-pkg pkg=glasschain-network
	RUST_TEST_THREADS=$(RUST_TEST_THREADS) cargo test -p $(pkg)

test-one: ## Run one test by substring: make test-one test=mine
	cargo test $(CARGO_FLAGS) -- $(test)

## ── Lint & quality ──────────────────────────────────────────────────────────

fmt: ## Format the entire workspace (writes changes; prefer scoping to touched files)
	cargo fmt --all

fmt-check: ## Verify formatting without modifying files (CI gate)
	cargo fmt --all --check

clippy: ## Run clippy with warnings as errors (CI gate)
	cargo clippy $(CARGO_FLAGS) -- -D warnings

bench: ## Run the criterion benchmarks
	cargo bench -p glasschain-vm
	cargo bench -p glasschain-workflows

audit: ## Audit dependencies for known vulnerabilities (run `make tools` first)
	cargo audit --deny warnings --file Cargo.lock

coverage: ## Generate an HTML coverage report (run `make tools` first)
	cargo tarpaulin --verbose --workspace --all-features --all-targets --locked --timeout 120 --out html

coverage-xml: ## Generate Cobertura XML coverage (same command CI uses)
	cargo tarpaulin --verbose --workspace --all-features --all-targets --locked --timeout 120 --out xml

## ── Aggregate ───────────────────────────────────────────────────────────────

ci: ## Run the CI gates in order: fmt-check -> clippy -> check -> test
	$(MAKE) fmt-check
	$(MAKE) clippy
	$(MAKE) check
	$(MAKE) test

## ── Run & docs ──────────────────────────────────────────────────────────────

node: ## Run a node REPL: make node id=node-1 port=8000 (interactive)
	@test -n "$(id)" || (echo "usage: make node id=node-1 port=8000"; exit 1)
	@test -n "$(port)" || (echo "usage: make node id=node-1 port=8000"; exit 1)
	cargo run --release -p glasschain-node -- --id $(id) --listen 0.0.0.0:$(port)

doc: ## Build the crate documentation
	cargo doc --workspace --no-deps

clean: ## Remove build artifacts (target/)
	cargo clean
