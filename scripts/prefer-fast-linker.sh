#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 dbbvitor
#
# Select the preferred fast linker for Rust builds (AGENTS.md / ADR-019):
# `wild` first, `mold` where wild has no build for the platform (e.g. aarch64
# Linux has no release artifact). Both are selected through clang: gcc only
# accepts `-fuse-ld=wild` from 16.1, while clang's `--ld-path` and
# `-fuse-ld=mold` work on any gcc. Linux only: both cover Linux ELF —
# wild has no aarch64 Linux release, mold has x86_64/aarch64/arm/riscv64/...
# — but neither links Mach-O or PE, so macOS/Windows keep their default
# linker. No-op when clang or both linkers are absent.
#
# In GitHub Actions (GITHUB_ENV set) the flags are appended to RUSTFLAGS for
# the following steps; anywhere else they are printed on stdout.
#
# Local check: scripts/prefer-fast-linker.sh --self-test
set -euo pipefail

select_flags() {
    if [ "$(uname -s)" != "Linux" ] || ! command -v clang >/dev/null 2>&1; then
        return 0
    fi
    if command -v wild >/dev/null 2>&1; then
        printf -- '-C linker=clang -C link-arg=--ld-path=%s' "$(command -v wild)"
    elif command -v mold >/dev/null 2>&1; then
        printf -- '-C linker=clang -C link-arg=-fuse-ld=mold'
    fi
}

if [ "${1:-}" = "--self-test" ]; then
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    mkdir -p "$tmp/bin"
    ln -s "$(command -v uname)" "$tmp/bin/uname"
    fake() {
        printf '#!/bin/sh\nexit 0\n' >"$tmp/bin/$1"
        chmod +x "$tmp/bin/$1"
    }
    run() { PATH="$tmp/bin" "$BASH" "$0"; }
    check() {
        local desc="$1" want="$2" got
        got=$(run)
        if [ "$got" != "$want" ]; then
            echo "self-test FAIL: $desc: want '$want', got '$got'" >&2
            exit 1
        fi
        echo "self-test ok: $desc"
    }

    fake wild
    check "no clang -> default" ""
    fake clang
    check "clang + wild -> wild" "-C linker=clang -C link-arg=--ld-path=$tmp/bin/wild"
    rm "$tmp/bin/wild"
    fake mold
    check "clang + mold -> mold" "-C linker=clang -C link-arg=-fuse-ld=mold"

    # GITHUB_ENV mode: append to the existing flags (the CI path).
    mkdir -p "$tmp/env"
    RUSTFLAGS="-D warnings" GITHUB_ENV="$tmp/env/vars" PATH="$tmp/bin" \
        "$BASH" "$0" >/dev/null
    want="RUSTFLAGS=-D warnings -C linker=clang -C link-arg=-fuse-ld=mold"
    got=$(cat "$tmp/env/vars")
    if [ "$got" != "$want" ]; then
        echo "self-test FAIL: GITHUB_ENV append: want '$want', got '$got'" >&2
        exit 1
    fi
    echo "self-test ok: GITHUB_ENV append"

    rm "$tmp/bin/mold"
    check "clang only -> default" ""
    echo "self-test passed"
    exit 0
fi

flags=$(select_flags)
if [ -z "$flags" ]; then
    exit 0
fi

if [ -n "${GITHUB_ENV:-}" ]; then
    if [ -n "${RUSTFLAGS:-}" ]; then
        echo "RUSTFLAGS=${RUSTFLAGS} $flags" >>"$GITHUB_ENV"
    else
        echo "RUSTFLAGS=$flags" >>"$GITHUB_ENV"
    fi
    echo "preferred linker: $flags"
else
    echo "$flags"
fi
