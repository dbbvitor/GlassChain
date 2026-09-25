#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 dbbvitor
#
# Print the workspace packages a PR diff can affect: the changed crates plus
# every crate that transitively depends on them (the reverse-dependency
# closure), so a package-scoped check never misses a dependent.
#
# Output (stdout):
#   ALL            a workspace-level file changed (manifests, lockfile,
#                  toolchain, lint/dependency config, .cargo, .config, typos
#                  config, or CI config) — run the full workspace
#   NONE           no crate is affected — skip the package-scoped checks
#   <names...>     space-separated package names (the closure)
#
# Usage: affected-crates.sh <base-rev> [--cargo-flags]
#   --cargo-flags  print `--workspace` / `-p a -p b` / an empty string for
#                  direct interpolation into a cargo command
#
# Local check: scripts/affected-crates.sh origin/main
set -euo pipefail

base="${1:?usage: affected-crates.sh <base-rev> [--cargo-flags]}"
mode="${2:-}"

# `--self-test` exercises the closure logic against synthetic diffs.
if [ "$base" = "--self-test" ]; then
  fail=0
  expect() {
    local desc="$1" diff="$2" want="$3" got
    got=$(AFFECTED_DIFF="$diff" "$0" HEAD)
    if [ "$got" != "$want" ]; then
      echo "FAIL: $desc: got '$got', want '$want'"
      fail=1
    else
      echo "ok: $desc"
    fi
  }
  expect "cli only" "crates/glasschain-cli/src/main.rs" "glasschain-cli"
  expect "node only" "crates/glasschain-node/src/main.rs" "glasschain-node"
  expect "docs only" "docs/readme.md" "NONE"
  expect "lockfile" "Cargo.lock" "ALL"
  expect "the CI script itself" "scripts/affected-crates.sh" "ALL"
  expect "the Makefile" "Makefile" "ALL"
  core=$(AFFECTED_DIFF="crates/glasschain-core/src/lib.rs" "$0" HEAD)
  all=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[].name' | sort | tr '\n' ' ' | xargs)
  if [ "$core" != "$all" ]; then
    echo "FAIL: core closure: got '$core', want '$all'"
    fail=1
  else
    echo "ok: core closure covers the workspace"
  fi
  exit "$fail"
fi

# `AFFECTED_DIFF` overrides the git diff (a newline-separated path list) so the
# closure logic can be exercised without a real commit range.
if [ -n "${AFFECTED_DIFF:-}" ]; then
  changed="$AFFECTED_DIFF"
else
  changed=$(git diff --name-only "$base"...HEAD)
fi

if printf '%s\n' "$changed" | grep -qE '^(Cargo\.(toml|lock)|rust-toolchain\.toml|clippy\.toml|deny\.toml|Makefile|scripts/|\.cargo/|\.config/|\.typos\.toml|\.github/)'; then
  packages=ALL
else
  dirs=$(printf '%s\n' "$changed" | grep -oE '^crates/[^/]+' | sort -u || true)
  if [ -z "$dirs" ]; then
    packages=NONE
  else
    metadata=$(cargo metadata --no-deps --format-version 1)
    # `name dir` for every workspace package, dir relative to the root.
    root=$(jq -r '.workspace_root' <<<"$metadata")
    names=$(jq -r --arg root "$root/" '.packages[] | .name + " " + (.manifest_path | sub($root; "") | sub("/Cargo.toml$"; ""))' <<<"$metadata")
    selected=""
    while IFS= read -r dir; do
      name=$(awk -v dir="$dir" '$2 == dir { print $1 }' <<<"$names")
      [ -n "$name" ] && selected="$selected $name"
    done <<<"$dirs"
    # `dependency dependent` for every path dependency edge.
    edges=$(jq -r '.packages[] | .name as $dependent | .dependencies[] | select(.path != null) | "\(.name) \($dependent)"' <<<"$metadata")
    # Fixed point: add every dependent of a selected package.
    all=" $selected "
    while :; do
      more=$(printf '%s\n' "$edges" | awk -v sel="$all" '{ if (index(sel, " " $1 " ") > 0 && index(sel, " " $2 " ") == 0) print $2 }' | sort -u | tr '\n' ' ')
      [ -z "$more" ] && break
      all="$all$more "
    done
    packages=$(echo "$all" | tr ' ' '\n' | sort -u | tr '\n' ' ' | xargs)
  fi
fi

case "$mode" in
  --cargo-flags)
    case "$packages" in
      ALL) echo "--workspace" ;;
      NONE) echo "" ;;
      *) for pkg in $packages; do printf -- '-p %s ' "$pkg"; done; echo "" ;;
    esac
    ;;
  *)
    echo "$packages"
    ;;
esac
